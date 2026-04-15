$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repo = if ($env:DEX_REPO) { $env:DEX_REPO } else { 'francescoalemanno/dex' }
$Binary = if ($env:DEX_BINARY) { $env:DEX_BINARY } else { 'dex.exe' }
$InstallDir = if ($env:INSTALL_DIR) {
    $env:INSTALL_DIR
} else {
    Join-Path $env:LOCALAPPDATA 'Programs\dex\bin'
}

$headers = @{
    'User-Agent' = 'dex-install-script'
}

$FallbackArch = $null

switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
    'X64' {
        $Arch = 'amd64'
        $DisplayArch = 'amd64'
    }
    'Arm64' {
        $Arch = 'arm64'
        $FallbackArch = 'amd64'
        $DisplayArch = 'arm64'
    }
    default { throw "Unsupported architecture: $([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture)" }
}

$release = Invoke-RestMethod -Headers $headers -Uri "https://api.github.com/repos/$Repo/releases/latest"
$Tag = $release.tag_name
if (-not $Tag) {
    throw 'Could not determine the latest release tag.'
}

$Version = $Tag.TrimStart('v')
$Archive = "dex_${Version}_windows_${Arch}.zip"
$Url = "https://github.com/$Repo/releases/download/$Tag/$Archive"

Write-Host "Installing dex $Tag (windows/$DisplayArch)..."

$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("dex-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $TempDir | Out-Null

try {
    $ZipPath = Join-Path $TempDir $Archive
    try {
        Invoke-WebRequest -Headers $headers -Uri $Url -OutFile $ZipPath
    }
    catch {
        if (-not $FallbackArch) {
            throw
        }

        $Archive = "dex_${Version}_windows_${FallbackArch}.zip"
        $Url = "https://github.com/$Repo/releases/download/$Tag/$Archive"
        $ZipPath = Join-Path $TempDir $Archive
        Write-Host "No native windows/arm64 release asset found, using amd64."
        Invoke-WebRequest -Headers $headers -Uri $Url -OutFile $ZipPath
    }
    Expand-Archive -LiteralPath $ZipPath -DestinationPath $TempDir -Force

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $TempDir $Binary) -Destination (Join-Path $InstallDir $Binary) -Force

    $UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $PathEntries = @()
    if ($UserPath) {
        $PathEntries = $UserPath.Split(';', [System.StringSplitOptions]::RemoveEmptyEntries)
    }

    if (-not ($PathEntries -contains $InstallDir)) {
        $NewPath = if ($UserPath) { "$InstallDir;$UserPath" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable('Path', $NewPath, 'User')
        Write-Host "Added $InstallDir to your user PATH. Restart your terminal to use it."
    }

    Write-Host "Installed dex $Tag to $(Join-Path $InstallDir $Binary)"
}
finally {
    Remove-Item -LiteralPath $TempDir -Recurse -Force -ErrorAction SilentlyContinue
}
