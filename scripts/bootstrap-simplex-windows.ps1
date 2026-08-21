$ErrorActionPreference = "Stop"

$RootDir = Split-Path -Parent $PSScriptRoot
$SourceDir = Join-Path $RootDir "vendor\simplex-chat"
$Revision = "ec6e975001861d494360cda4aa267747d3a14272"
$GhcVersion = "9.6.3"
$CabalVersion = "3.16.1.0"

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "The SimpleX Windows build requires 64-bit Windows."
}

function Install-GhcupEnvironment {
    Write-Host "Installing missing GHCup/MSYS2 components..."
    $bootstrap = Invoke-WebRequest "https://www.haskell.org/ghcup/sh/bootstrap-haskell.ps1" -UseBasicParsing
    $bootstrapContent = $bootstrap.Content
    if ($bootstrapContent -is [byte[]]) {
        $bootstrapContent = [Text.Encoding]::UTF8.GetString($bootstrapContent)
    }
    & ([ScriptBlock]::Create([string] $bootstrapContent)) -Minimal -InBash -Msys2Env UCRT64 -DontWriteDesktopShortcuts
}

$Ghcup = Get-Command ghcup.exe -ErrorAction SilentlyContinue
$MsysDir = [Environment]::GetEnvironmentVariable("GHCUP_MSYS2", "User")
if (-not $MsysDir) { $MsysDir = Join-Path ${env:SystemDrive} "ghcup\msys64" }
$Bash = Join-Path $MsysDir "usr\bin\bash.exe"

# GitHub's Windows image may provide ghcup.exe without the companion MSYS2
# installation. Run the bootstrap when either component is missing; it keeps an
# existing GHCup installation and installs MSYS2 into the configured location.
if (-not $Ghcup -or -not (Test-Path $Bash)) {
    Install-GhcupEnvironment
    $Ghcup = Get-Command ghcup.exe -ErrorAction SilentlyContinue
    $MsysDir = [Environment]::GetEnvironmentVariable("GHCUP_MSYS2", "User")
    if (-not $MsysDir) { $MsysDir = Join-Path ${env:SystemDrive} "ghcup\msys64" }
    $Bash = Join-Path $MsysDir "usr\bin\bash.exe"
}
if (-not $Ghcup) {
    $GhcupPath = Join-Path ${env:SystemDrive} "ghcup\bin\ghcup.exe"
    if (Test-Path $GhcupPath) { $Ghcup = Get-Command $GhcupPath }
}
if (-not $Ghcup) { throw "GHCup installation did not produce ghcup.exe." }
$GhcupExe = $Ghcup.Source

function Test-GhcupToolInstalled {
    param(
        [Parameter(Mandatory)] [string] $Tool,
        [Parameter(Mandatory)] [string] $Version
    )

    # `ghcup whereis` reports a missing tool on stderr. Windows PowerShell turns
    # that expected result into a terminating NativeCommandError while the
    # script-wide ErrorActionPreference is Stop, so relax it for this probe.
    $PreviousErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        & $GhcupExe whereis $Tool $Version *> $null
        return $LASTEXITCODE -eq 0
    }
    finally {
        $ErrorActionPreference = $PreviousErrorActionPreference
    }
}

if (-not (Test-GhcupToolInstalled -Tool "ghc" -Version $GhcVersion)) {
    & $GhcupExe install ghc $GhcVersion --no-set
}
if (-not (Test-GhcupToolInstalled -Tool "cabal" -Version $CabalVersion)) {
    & $GhcupExe install cabal $CabalVersion --no-set
}
& $GhcupExe set ghc $GhcVersion
if ($LASTEXITCODE -ne 0) { throw "Selecting GHC $GhcVersion failed." }
& $GhcupExe set cabal $CabalVersion
if ($LASTEXITCODE -ne 0) { throw "Selecting Cabal $CabalVersion failed." }

if (-not (Test-Path $Bash)) { throw "MSYS2 bash was not found at $Bash." }

$env:MSYSTEM = "UCRT64"
$env:CHERE_INVOKING = "1"
& $Bash -lc "pacman --noconfirm -S --needed git perl make mingw-w64-ucrt-x86_64-cmake mingw-w64-ucrt-x86_64-gcc"
if ($LASTEXITCODE -ne 0) { throw "Installing MSYS2 build dependencies failed." }

if (-not (Test-Path (Join-Path $SourceDir ".git"))) {
    New-Item -ItemType Directory -Force (Split-Path -Parent $SourceDir) | Out-Null
    git clone https://github.com/simplex-chat/simplex-chat.git $SourceDir
}
git -C $SourceDir fetch origin stable
git -C $SourceDir checkout --detach $Revision

$env:SIMPLEX_ROOT = $RootDir
$env:SIMPLEX_GHCUP = $GhcupExe
$env:SIMPLEX_GHC = (& $GhcupExe whereis ghc $GhcVersion).Trim()
$env:SIMPLEX_CABAL = (& $GhcupExe whereis cabal $CabalVersion).Trim()
$env:SIMPLEX_GHC_BIN = Split-Path -Parent $env:SIMPLEX_GHC
$env:SIMPLEX_CABAL_BIN = Split-Path -Parent $env:SIMPLEX_CABAL
$RootUnix = (& $Bash -lc 'cygpath -u "$SIMPLEX_ROOT"').Trim()
$GhcBinUnix = (& $Bash -lc 'cygpath -u "$SIMPLEX_GHC_BIN"').Trim()
$CabalBinUnix = (& $Bash -lc 'cygpath -u "$SIMPLEX_CABAL_BIN"').Trim()
$build = "export PATH='${GhcBinUnix}:${CabalBinUnix}:/ucrt64/bin':`$PATH; cd '$RootUnix/vendor/simplex-chat'; cabal update && bash scripts/desktop/build-lib-windows.sh"
& $Bash -lc $build
if ($LASTEXITCODE -ne 0) { throw "The upstream SimpleX Windows build failed." }

$BuildDir = Join-Path $SourceDir "apps\multiplatform\common\src\commonMain\cpp\desktop\libs\windows-x86_64"
$InstallDir = Join-Path $RootDir "vendor\libsimplex"
if (-not (Test-Path (Join-Path $BuildDir "libsimplex.dll"))) { throw "libsimplex.dll was not produced." }
New-Item -ItemType Directory -Force $InstallDir | Out-Null
Copy-Item (Join-Path $BuildDir "*.dll") $InstallDir -Force
Write-Host "Built $InstallDir\libsimplex.dll"
