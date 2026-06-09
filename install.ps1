# Helix CLI Windows Installer
# One-liner to download, install, and launch Helix on Windows natively.
# Usage (PowerShell): irm https://raw.githubusercontent.com/poornachandra24/Helix/main/install.ps1 | iex

$ErrorActionPreference = 'Stop'

# Define ANSI colors for PowerShell
$Blue = "`e[0;34m"
$Cyan = "`e[0;36m"
$Green = "`e[0;32m"
$Yellow = "`e[0;33m"
$Red = "`e[0;31m"
$Bold = "`e[1m"
$NC = "`e[0m"

Write-Host "${Blue}╭────────────────────────────────────────────────────────╮${NC}"
Write-Host "${Blue}│${NC}    ${Bold}${Cyan}Helix — Autonomous Tool-Calling Agent CLI${NC}         ${Blue}│${NC}"
Write-Host "${Blue}├────────────────────────────────────────────────────────┤${NC}"
Write-Host "${Blue}│${NC}  Installing the latest precompiled Windows binary...    ${Blue}│${NC}"
Write-Host "${Blue}╰────────────────────────────────────────────────────────╯${NC}`n"

$Repo = "poornachandra24/helix"
$GithubApi = "https://api.github.com/repos/$Repo/releases/latest"

# Detect System Architecture
$Arch = "x86_64"
if ([System.Environment]::Is64BitOperatingSystem) {
    # Check if ARM64
    $ProcessorArch = $env:PROCESSOR_ARCHITECTURE
    if ($ProcessorArch -eq "ARM64") {
        $Arch = "aarch64"
    }
} else {
    Write-Error "Error: 32-bit Windows is not supported."
    exit 1
}

$TargetTriplet = "${Arch}-pc-windows-msvc"
Write-Host "${Cyan}• Detecting system:${NC} Windows (${Arch}) -> $TargetTriplet"

# Query GitHub Release
Write-Host "${Cyan}• Querying latest release from GitHub...${NC}"
try {
    $ReleaseInfo = Invoke-RestMethod -Uri $GithubApi -UseBasicParsing
    $Tag = $ReleaseInfo.tag_name
    # Search for asset containing target triplet
    $Asset = $ReleaseInfo.assets | Where-Object { $_.name -like "*$TargetTriplet*" } | Select-Object -First 1
    $DownloadUrl = $Asset.browser_download_url
} catch {
    Write-Host "${Yellow}Warning: Failed to fetch release details from API. Using fallback download path.${NC}"
    $Tag = "latest"
    $DownloadUrl = "https://github.com/$Repo/releases/latest/download/helix-$TargetTriplet.zip"
}

if (-not $DownloadUrl) {
    # Default zip fallback
    $DownloadUrl = "https://github.com/$Repo/releases/download/$Tag/helix-$TargetTriplet.zip"
}

Write-Host "${Cyan}• Version:${NC} ${Bold}$Tag${NC}"
Write-Host "${Cyan}• Downloading:${NC} $DownloadUrl"

# Temporary directory path
$TempDir = Join-Path $env:TEMP "helix-installer"
if (Test-Path $TempDir) { Remove-Item -Recurse -Force $TempDir }
$null = New-Item -ItemType Directory -Path $TempDir

$ZipFile = Join-Path $TempDir "helix.zip"

# Download Zip
try {
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $ZipFile -UseBasicParsing
} catch {
    Write-Error "Error: Failed to download release asset from $DownloadUrl"
    exit 1
}

# Extract Zip
Write-Host "${Cyan}• Extracting archive...${NC}"
Expand-Archive -Path $ZipFile -DestinationPath $TempDir -Force

# Locate executable
$BinaryPath = Get-ChildItem -Path $TempDir -Filter "helix.exe" -Recurse | Select-Object -ExpandProperty FullName -First 1

if (-not $BinaryPath -or -not (Test-Path $BinaryPath)) {
    Write-Error "Error: Could not find 'helix.exe' in extracted archive."
    exit 1
}

# Install Directory
$InstallDir = Join-Path $env:USERPROFILE ".helix\bin"
if (-not (Test-Path $InstallDir)) {
    $null = New-Item -ItemType Directory -Path $InstallDir
}

$DestFile = Join-Path $InstallDir "helix.exe"
Write-Host "${Cyan}• Installing binary to:${NC} ${Bold}$DestFile${NC}"
Copy-Item -Path $BinaryPath -Destination $DestFile -Force

Write-Host "${Green}✓ Installation successful!${NC}`n"

# Verify PATH environment variable
$UserPath = [System.Environment]::GetEnvironmentVariable("PATH", "User")
$PathSegments = $UserPath -split ";"
$PathInPath = $false
foreach ($Segment in $PathSegments) {
    if ($Segment.TrimEnd('\') -eq $InstallDir.TrimEnd('\')) {
        $PathInPath = $true
        break
    }
}

if (-not $PathInPath) {
    Write-Host "${Yellow}⚠️ Notice: '$InstallDir' is not in your User PATH.${NC}"
    Write-Host "Adding to Environment PATH..."
    $NewUserPath = $UserPath + ";" + $InstallDir
    [System.Environment]::SetEnvironmentVariable("PATH", $NewUserPath, "User")
    # Also update current process path so it is runnable immediately
    $env:PATH = $env:PATH + ";" + $InstallDir
    Write-Host "${Green}✓ Path updated successfully.${NC}`n"
}

# Prompt to launch setup
$Response = Read-Host "Would you like to run Helix setup now? (y/n)"
if ($Response -match "^[yY](es)?$") {
    Write-Host "`n${Cyan}Starting Helix...${NC}`n"
    & "$DestFile"
} else {
    Write-Host "`nTo get started, open a new terminal and run:"
    Write-Host "  ${Bold}helix${NC}`n"
    Write-Host "Have fun with your self-evolving CLI agent! 🚀`n"
}
