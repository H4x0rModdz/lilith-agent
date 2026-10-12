"""Servidor de voz da Lilith: Qwen3-TTS (voz "Serena") na GPU.

Fala com o app por linhas JSON em stdin/stdout. Só linhas que começam com o
prefixo @@LILITH@@ são respostas; todo o resto (avisos das bibliotecas) vai para
stderr. Um pedido por vez:

  entrada : {"id": 1, "text": "...", "speaker": "Serena", "instruct": "..."}
  resposta: @@LILITH@@ {"id": 1, "path": "C:/.../x.wav", "sr": 24000}

Ao terminar de carregar o modelo, envia {"ready": true, "vram_gb": 3.9}.
"""
import json
import os
import sys
import tempfile
import time

# stdout é só do protocolo: o resto das bibliotecas imprime em stderr.
PROTO = sys.stdout
sys.stdout = sys.stderr
PREFIX = "@@LILITH@@ "


def send(obj):
    PROTO.write(PREFIX + json.dumps(obj, ensure_ascii=False) + "\n")
    PROTO.flush()


def main():
    try:
        import soundfile as sf
        import torch
        from qwen_tts import Qwen3TTSModel

        model_id = os.environ.get("LILITH_QWEN_MODEL", "Qwen/Qwen3-TTS-12Hz-1.7B-CustomVoice")
        if not torch.cuda.is_available():
            send({"error": "sem GPU NVIDIA (CUDA) disponível"})
            return 2
        started = time.time()
        model = Qwen3TTSModel.from_pretrained(model_id, device_map="cuda:0", dtype=torch.bfloat16)
        send({
            "ready": True,
            "load_s": round(time.time() - started, 1),
            "vram_gb": round(torch.cuda.memory_allocated() / 2**30, 2),
            "gpu": torch.cuda.get_device_name(0),
        })
    except Exception as e:  # noqa: BLE001
        send({"error": f"não consegui iniciar o modelo: {e}"})
        return 1

    out_dir = os.path.join(tempfile.gettempdir(), "lilith-qwen")
    os.makedirs(out_dir, exist_ok=True)
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            continue
        if req.get("quit"):
            break
        rid = req.get("id")
        try:
            t = time.time()
            kwargs = {"text": req["text"], "language": req.get("language", "Portuguese"), "speaker": req.get("speaker", "Serena")}
            if req.get("instruct"):
                kwargs["instruct"] = req["instruct"]
            wavs, sr = model.generate_custom_voice(**kwargs)
            path = os.path.join(out_dir, f"{os.getpid()}-{rid}.wav")
            sf.write(path, wavs[0], sr)
            send({"id": rid, "path": path, "sr": sr, "gen_s": round(time.time() - t, 2)})
        except Exception as e:  # noqa: BLE001
            send({"id": rid, "error": str(e)})
    return 0


if __name__ == "__main__":
    sys.exit(main())
