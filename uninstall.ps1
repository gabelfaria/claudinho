# Claudinho - desinstalador / uninstaller
#   irm https://raw.githubusercontent.com/gabelfaria/claudinho/main/uninstall.ps1 | iex
# Remove o programa e os atalhos. Sua conversa, memoria e config ficam em %APPDATA%\Claudinho
# (apague a pasta se quiser). / Your data stays in %APPDATA%\Claudinho unless you delete it.

& {
Get-Process claudinho, claudinho-ui -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 800
$itens = @(
    (Join-Path $env:LOCALAPPDATA 'Programs\Claudinho'),
    (Join-Path $env:LOCALAPPDATA 'Claudinho'),
    (Join-Path ([Environment]::GetFolderPath('Programs')) 'Claudinho.lnk'),
    (Join-Path ([Environment]::GetFolderPath('Startup')) 'Claudinho.lnk')
)
foreach ($i in $itens) { if (Test-Path $i) { Remove-Item $i -Recurse -Force; Write-Host "removido: $i" } }
Write-Host ''
Write-Host "Claudinho removido. Seus dados continuam em $(Join-Path $env:APPDATA 'Claudinho')." -ForegroundColor Green
}
