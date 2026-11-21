# Claudinho - instalador / installer
#   irm https://raw.githubusercontent.com/gabelfaria/claudinho/main/install.ps1 | iex
# Instala o Claude Code (se faltar), baixa o Claudinho, cria os atalhos e abre.
# Installs Claude Code (if missing), downloads Claudinho, creates shortcuts and starts it.
# Opcional / optional:  $env:CLAUDINHO_ZIP = 'C:\caminho\Claudinho-windows-x64.zip'  (instala de um zip local)

& {
$ErrorActionPreference = 'Stop'
$repo    = 'gabelfaria/claudinho'
$destino = Join-Path $env:LOCALAPPDATA 'Programs\Claudinho'
$config  = Join-Path $env:APPDATA 'Claudinho\config.json'

function Passo($t) { Write-Host "`n> $t" -ForegroundColor Cyan }
function Ok($t)    { Write-Host "  $t" -ForegroundColor Green }
function Aviso($t) { Write-Host "  $t" -ForegroundColor Yellow }

Write-Host ''
Write-Host '  Claudinho - seu assistente de mesa com Claude Code' -ForegroundColor Cyan
Write-Host '  Projeto independente, nao oficial da Anthropic.' -ForegroundColor DarkGray

if ([Environment]::OSVersion.Version.Major -lt 10 -or -not [Environment]::Is64BitOperatingSystem) {
    throw 'O Claudinho precisa de Windows 10 ou 11 (64 bits). / Requires Windows 10/11 x64.'
}
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

# 1) Claude Code
Passo 'Claude Code'
$claude = Join-Path $env:USERPROFILE '.local\bin\claude.exe'
if (-not (Test-Path $claude)) {
    $cmd = Get-Command claude -ErrorAction SilentlyContinue
    if ($cmd) { $claude = $cmd.Source }
}
if (-not (Test-Path $claude)) {
    Aviso 'Nao encontrei o Claude Code. Instalando pelo instalador oficial (claude.ai/install.ps1)...'
    Invoke-RestMethod https://claude.ai/install.ps1 | Invoke-Expression
    $claude = Join-Path $env:USERPROFILE '.local\bin\claude.exe'
    if (-not (Test-Path $claude)) { throw 'O Claude Code nao foi instalado. Veja https://docs.claude.com/claude-code' }
}
Ok "Claude Code: $claude"
$logado = $false
try { $s = & $claude auth status 2>&1 | Out-String; $logado = $s -match '"loggedIn"\s*:\s*true' } catch {}
if (-not $logado) {
    Aviso 'Entre com a sua conta Claude (Pro/Max). Vai abrir o navegador...'
    & $claude auth login
}

# 2) Download
Passo 'Baixando o Claudinho'
$tmp = Join-Path $env:TEMP ('claudinho-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $tmp | Out-Null
$zip = Join-Path $tmp 'Claudinho.zip'
if ($env:CLAUDINHO_ZIP) {
    Copy-Item $env:CLAUDINHO_ZIP $zip
    Ok "zip local: $env:CLAUDINHO_ZIP"
} else {
    $rel = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest" -Headers @{ 'User-Agent' = 'claudinho-installer' }
    $asset = $rel.assets | Where-Object { $_.name -like '*windows-x64.zip' } | Select-Object -First 1
    if (-not $asset) { throw "Nao achei o zip na release $($rel.tag_name)." }
    Invoke-WebRequest $asset.browser_download_url -OutFile $zip -UseBasicParsing
    Ok "versao $($rel.tag_name)"
}

# 3) Instala (fecha a versao antiga, se estiver aberta)
Passo "Instalando em $destino"
Get-Process claudinho, claudinho-ui -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 800
New-Item -ItemType Directory -Force $destino | Out-Null
Expand-Archive $zip -DestinationPath $tmp\x -Force
Get-ChildItem "$tmp\x" -Recurse -File | ForEach-Object { Copy-Item $_.FullName (Join-Path $destino $_.Name) -Force }
Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
Ok 'arquivos copiados'

# 4) Primeira configuracao: como te chamar
if (-not (Test-Path $config)) {
    $nome = ''
    try { $nome = Read-Host '  Como o Claudinho deve te chamar? (Enter = nome do Windows)' } catch {}
    if (-not $nome) { $nome = $env:USERNAME }
    $pasta = Split-Path $config
    New-Item -ItemType Directory -Force $pasta | Out-Null
    $cfg = [ordered]@{
        nome           = $nome
        pasta_trabalho = $env:USERPROFILE
        pasta_dados    = (Join-Path $pasta 'dados')
        prompt         = (Join-Path $pasta 'prompt.md')
    }
    [IO.File]::WriteAllText($config, ($cfg | ConvertTo-Json), (New-Object Text.UTF8Encoding $false))
    Ok "config: $config"
}

# 5) Atalhos: menu Iniciar e iniciar com o Windows
Passo 'Atalhos'
$sh = New-Object -ComObject WScript.Shell
$alvo = Join-Path $destino 'claudinho.exe'
foreach ($lnk in @(
    (Join-Path ([Environment]::GetFolderPath('Programs')) 'Claudinho.lnk'),
    (Join-Path ([Environment]::GetFolderPath('Startup')) 'Claudinho.lnk'))) {
    $a = $sh.CreateShortcut($lnk)
    $a.TargetPath = $alvo
    $a.WorkingDirectory = $destino
    $a.Description = 'Claudinho - assistente de mesa com Claude Code'
    $a.Save()
}
Ok 'menu Iniciar + iniciar com o Windows'

# 6) Abre
Start-Process $alvo
Write-Host ''
Write-Host '  Pronto! A bolhinha do Claudinho apareceu no canto da tela. Clique nela para conversar.' -ForegroundColor Green
Write-Host '  Ready! Click the Claudinho bubble on your screen to start chatting.' -ForegroundColor Green
Write-Host "  Personalize: $config  (nome, pastas) e o prompt.md ao lado." -ForegroundColor DarkGray
Write-Host ''
}
