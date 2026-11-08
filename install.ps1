# Install claude-statusline into %USERPROFILE%\.claude\bin on Windows.
# Builds from this checkout when cargo is available, otherwise downloads the
# latest release binary ($env:CLAUDE_STATUSLINE_DOWNLOAD = '1' forces the
# download). A running copy is moved aside, not overwritten.
$ErrorActionPreference = 'Stop'

$dest = if ($env:CLAUDE_STATUSLINE_DIR) { $env:CLAUDE_STATUSLINE_DIR } else { Join-Path $HOME '.claude\bin' }
$exe = Join-Path $dest 'claude-statusline.exe'
$releases = 'https://github.com/ductran27/claude-code-statusline/releases/latest/download'
$downloaded = $false

$build = (Get-Command cargo -ErrorAction SilentlyContinue) -and (Test-Path (Join-Path $PSScriptRoot 'Cargo.toml')) -and ($env:CLAUDE_STATUSLINE_DOWNLOAD -ne '1')
if ($build) {
    Write-Host 'Building with cargo'
    cargo build --release --locked --manifest-path (Join-Path $PSScriptRoot 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
    $src = Join-Path $PSScriptRoot 'target\release\claude-statusline.exe'
} else {
    # x86_64 also runs on ARM64 Windows through emulation.
    $arch = 'x86_64'
    $src = Join-Path ([IO.Path]::GetTempPath()) "claude-statusline-$PID.exe"
    Write-Host "Downloading claude-statusline-$arch-pc-windows-msvc.exe"
    # Windows PowerShell 5.1 may default to TLS 1.0, which GitHub refuses.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -UseBasicParsing -Uri "$releases/claude-statusline-$arch-pc-windows-msvc.exe" -OutFile $src
    $downloaded = $true
}

New-Item -ItemType Directory -Force -Path $dest | Out-Null
$stale = Join-Path $dest 'claude-statusline.exe.old*'
Remove-Item -Force $stale -ErrorAction SilentlyContinue
if (Test-Path $exe) { Move-Item $exe "$exe.old.$PID" }
Copy-Item -Force $src $exe
Remove-Item -Force $stale -ErrorAction SilentlyContinue
if ($downloaded) { Remove-Item -Force $src -ErrorAction SilentlyContinue }

$cmd = $exe -replace '\\', '/'
Write-Host "Installed $exe"
Write-Host ''
Write-Host 'Point Claude Code at it in %USERPROFILE%\.claude\settings.json:'
Write-Host ''
Write-Host '  "statusLine": {'
Write-Host '    "type": "command",'
Write-Host "    `"command`": `"$cmd`","
Write-Host '    "refreshInterval": 60'
Write-Host '  }'
