<#
Publica uma versão: compila o instalador assinado, gera o latest.json do
atualizador e cria o release no GitHub.

Uso (na raiz do repositório):
  .\scripts\release.ps1 -Notes "o que mudou"

Pré-requisitos: gh autenticado e a chave de assinatura em
$env:USERPROFILE\.tauri\lilith.key (nunca vai para o repositório).
A versão vem de app/src-tauri/tauri.conf.json.
#>
param(
  [Parameter(Mandatory = $true)][string]$Notes,
  [string]$KeyPath = "$env:USERPROFILE\.tauri\lilith.key",
  [switch]$Draft
)

$ErrorActionPreference = "Stop"
$conf = Get-Content app/src-tauri/tauri.conf.json -Raw | ConvertFrom-Json
$version = $conf.version
$tag = "v$version"
$repo = "H4x0rModdz/lilith-agent"

if (-not (Test-Path $KeyPath)) { throw "Chave de assinatura não encontrada em $KeyPath" }
if (git status --porcelain) { throw "Há alterações não commitadas; faça commit antes de publicar." }

$env:TAURI_SIGNING_PRIVATE_KEY = $KeyPath
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
Push-Location app
npm run tauri build
if ($LASTEXITCODE -ne 0) { Pop-Location; throw "Falha no build" }
Pop-Location

$dir = "target/release/bundle/nsis"
$setup = "$dir/Lilith_${version}_x64-setup.exe"
$sig = "$setup.sig"
if (-not (Test-Path $setup) -or -not (Test-Path $sig)) { throw "Instalador ou assinatura não encontrados em $dir" }

$latest = @{
  version  = $version
  notes    = $Notes
  pub_date = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
  platforms = @{
    "windows-x86_64" = @{
      signature = (Get-Content $sig -Raw).Trim()
      url       = "https://github.com/$repo/releases/download/$tag/Lilith_${version}_x64-setup.exe"
    }
  }
}
$latestPath = "$dir/latest.json"
$latest | ConvertTo-Json -Depth 5 | Set-Content $latestPath -Encoding utf8

$ghArgs = @("release", "create", $tag, $setup, $latestPath, "--title", "Lilith $tag", "--notes", $Notes, "--latest")
if ($Draft) { $ghArgs += "--draft" }
git tag -a $tag -m $tag
git push origin $tag
gh @ghArgs
Write-Host "Publicado: https://github.com/$repo/releases/tag/$tag"
