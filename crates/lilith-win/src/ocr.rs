//! Ler a tela por imagem: captura uma janela ou um monitor e reconhece o
//! texto com o OCR do próprio Windows (Windows.Media.Ocr — local, gratuito,
//! sem modelo extra). Serve para o que a acessibilidade não alcança: jogos,
//! imagens, PDFs escaneados, apps que não expõem texto.

use crate::window::Rect;
use crate::WinError;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HBITMAP, HDC, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};

/// Imagem BGRA (de cima para baixo).
pub struct Capture {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Lê os pixels de um bitmap GDI selecionado em `dc`.
unsafe fn read_bitmap(dc: HDC, bmp: HBITMAP, width: i32, height: i32) -> Result<Capture, WinError> {
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // negativo = de cima para baixo
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bgra = vec![0u8; (width * height * 4) as usize];
    let lines = GetDIBits(dc, bmp, 0, height as u32, Some(bgra.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
    if lines == 0 {
        return Err(WinError::Failed("não consegui ler os pixels da tela".into()));
    }
    Ok(Capture { bgra, width: width as u32, height: height as u32 })
}

/// Captura um retângulo da tela (coordenadas da área de trabalho virtual).
pub fn capture_rect(r: Rect) -> Result<Capture, WinError> {
    if r.w <= 0 || r.h <= 0 {
        return Err(WinError::InvalidParams("área vazia".into()));
    }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, r.w, r.h);
        let old = SelectObject(mem, bmp.into());
        let ok = BitBlt(mem, 0, 0, r.w, r.h, Some(screen), r.x, r.y, SRCCOPY | CAPTUREBLT).is_ok();
        let out = if ok { read_bitmap(mem, bmp, r.w, r.h) } else { Err(WinError::Failed("falha ao capturar a tela".into())) };
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        out
    }
}

/// Captura uma janela pelo conteúdo dela (mesmo se outra estiver por cima).
/// Se o app não desenhar para captura (fica tudo preto), usa o recorte da tela.
pub fn capture_window(id: isize, frame: Rect) -> Result<Capture, WinError> {
    const PW_RENDERFULLCONTENT: u32 = 2;
    let (w, h) = (frame.w.max(1), frame.h.max(1));
    let printed = unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        let ok = PrintWindow(HWND(id as *mut _), mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
        let out = if ok { read_bitmap(mem, bmp, w, h).ok() } else { None };
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        out
    };
    match printed {
        Some(c) if !is_blank(&c) => Ok(c),
        _ => capture_rect(frame),
    }
}

/// Tudo de uma cor só (janela que não se desenhou na captura)?
fn is_blank(c: &Capture) -> bool {
    let first = &c.bgra[..3.min(c.bgra.len())];
    c.bgra.chunks_exact(4).step_by(97).all(|p| &p[..3] == first)
}

/// Reduz a imagem por um fator inteiro (média dos pixels) — o OCR do Windows
/// tem tamanho máximo de imagem.
fn downscale(c: &Capture, factor: u32) -> Capture {
    if factor <= 1 {
        return Capture { bgra: c.bgra.clone(), width: c.width, height: c.height };
    }
    let (w, h) = (c.width / factor, c.height / factor);
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0u32; 4];
            for dy in 0..factor {
                for dx in 0..factor {
                    let i = (((y * factor + dy) * c.width + (x * factor + dx)) * 4) as usize;
                    for k in 0..4 {
                        acc[k] += c.bgra[i + k] as u32;
                    }
                }
            }
            let o = ((y * w + x) * 4) as usize;
            for k in 0..4 {
                out[o + k] = (acc[k] / (factor * factor)) as u8;
            }
        }
    }
    Capture { bgra: out, width: w, height: h }
}

/// Reconhece o texto (linhas, na ordem da tela).
pub fn recognize(c: &Capture) -> Result<Vec<String>, WinError> {
    use windows::core::HSTRING;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    let err = |e: windows::core::Error| WinError::Failed(format!("OCR: {e}"));
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    // Idioma do usuário; senão português; senão inglês.
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .ok()
        .or_else(|| Language::CreateLanguage(&HSTRING::from("pt-BR")).ok().and_then(|l| OcrEngine::TryCreateFromLanguage(&l).ok()))
        .or_else(|| Language::CreateLanguage(&HSTRING::from("en-US")).ok().and_then(|l| OcrEngine::TryCreateFromLanguage(&l).ok()))
        .ok_or_else(|| WinError::Unavailable("o OCR do Windows não tem idioma instalado".into()))?;
    let max = OcrEngine::MaxImageDimension().map_err(err)?.max(1);
    let factor = c.width.max(c.height).div_ceil(max).max(1);
    let img = downscale(c, factor);
    let buffer = CryptographicBuffer::CreateFromByteArray(&img.bgra).map_err(err)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, img.width as i32, img.height as i32)
        .map_err(err)?;
    let result = engine.RecognizeAsync(&bitmap).map_err(err)?.join().map_err(err)?;
    let mut lines = Vec::new();
    for line in result.Lines().map_err(err)? {
        let text = line.Text().map_err(err)?.to_string();
        if !text.trim().is_empty() {
            lines.push(text);
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Desenha um texto conhecido numa imagem em memória (sem tocar na tela)
    /// e confere o reconhecimento. `cargo test -- --ignored ocr_reads_drawn_text`.
    #[test]
    #[ignore]
    fn ocr_reads_drawn_text() {
        use windows::core::PCWSTR;
        use windows::Win32::Graphics::Gdi::{
            CreateFontW, CreateSolidBrush, FillRect, SetBkMode, SetTextColor, TextOutW, FONT_CHARSET,
            FONT_CLIP_PRECISION, FONT_OUTPUT_PRECISION, FONT_QUALITY, TRANSPARENT,
        };
        use windows::Win32::Foundation::{COLORREF, RECT};
        let (w, h) = (900, 120);
        let text: Vec<u16> = "Lilith reconhece texto 2026".encode_utf16().collect();
        let cap = unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            let bmp = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(mem, bmp.into());
            let white = CreateSolidBrush(COLORREF(0x00FF_FFFF));
            FillRect(mem, &RECT { left: 0, top: 0, right: w, bottom: h }, white);
            let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
            let font = CreateFontW(56, 0, 0, 0, 400, 0, 0, 0, FONT_CHARSET(0), FONT_OUTPUT_PRECISION(0), FONT_CLIP_PRECISION(0), FONT_QUALITY(5), 0, PCWSTR(face.as_ptr()));
            SelectObject(mem, font.into());
            SetBkMode(mem, TRANSPARENT);
            SetTextColor(mem, COLORREF(0));
            let _ = TextOutW(mem, 20, 30, &text);
            let c = read_bitmap(mem, bmp, w, h).unwrap();
            SelectObject(mem, old);
            ReleaseDC(None, screen);
            c
        };
        let lines = recognize(&cap).expect("OCR rodou");
        let all = lines.join(" ");
        assert!(all.contains("Lilith") && all.contains("2026"), "leu: {all:?}");
    }

    #[test]
    fn downscale_averages_blocks() {
        let c = Capture { bgra: vec![0, 0, 0, 255, 200, 200, 200, 255, 0, 0, 0, 255, 200, 200, 200, 255], width: 2, height: 2 };
        let d = downscale(&c, 2);
        assert_eq!((d.width, d.height), (1, 1));
        assert_eq!(d.bgra[..3], [100, 100, 100]);
        assert!(is_blank(&Capture { bgra: vec![9; 400], width: 10, height: 10 }));
    }
}
