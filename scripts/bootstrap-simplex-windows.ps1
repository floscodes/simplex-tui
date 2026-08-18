$ErrorActionPreference = "Stop"

$RootDir = Split-Path -Parent $PSScriptRoot
$SourceDir = Join-Path $RootDir "vendor\simplex-chat"
$Revision = "ec6e975001861d494360cda4aa267747d3a14272"
$GhcVersion = "9.6.3"
$CabalVersion = "3.10.1.0"

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "The SimpleX Windows build requires 64-bit Windows."
}

$Ghcup = Get-Command ghcup.exe -ErrorAction SilentlyContinue
if (-not $Ghcup) {
    Write-Host "Installing GHCup and MSYS2..."
    $bootstrap = Invoke-WebRequest "https://www.haskell.org/ghcup/sh/bootstrap-haskell.ps1" -UseBasicParsing
    & ([ScriptBlock]::Create($bootstrap.Content)) -Minimal -InBash -Msys2Env UCRT64 -DontWriteDesktopShortcuts
    $Ghcup = Get-Command ghcup.exe -ErrorAction SilentlyContinue
}
if (-not $Ghcup) {
    $GhcupPath = Join-Path ${env:SystemDrive} "ghcup\bin\ghcup.exe"
    if (Test-Path $GhcupPath) { $Ghcup = Get-Command $GhcupPath }
}
if (-not $Ghcup) { throw "GHCup installation did not produce ghcup.exe." }
$GhcupExe = $Ghcup.Source

& $GhcupExe whereis ghc $GhcVersion *> $null
if ($LASTEXITCODE -ne 0) { & $GhcupExe install ghc $GhcVersion --no-set }
& $GhcupExe whereis cabal $CabalVersion *> $null
if ($LASTEXITCODE -ne 0) { & $GhcupExe install cabal $CabalVersion --no-set }

$MsysDir = [Environment]::GetEnvironmentVariable("GHCUP_MSYS2", "User")
if (-not $MsysDir) { $MsysDir = Join-Path ${env:SystemDrive} "ghcup\msys64" }
$Bash = Join-Path $MsysDir "usr\bin\bash.exe"
if (-not (Test-Path $Bash)) { throw "MSYS2 bash was not found at $Bash." }

$env:MSYSTEM = "UCRT64"
$env:CHERE_INVOKING = "1"
& $Bash -lc "pacman --noconfirm -S --needed perl make mingw-w64-ucrt-x86_64-cmake mingw-w64-ucrt-x86_64-gcc"
if ($LASTEXITCODE -ne 0) { throw "Installing MSYS2 build dependencies failed." }

if (-not (Test-Path (Join-Path $SourceDir ".git"))) {
    New-Item -ItemType Directory -Force (Split-Path -Parent $SourceDir) | Out-Null
    git clone https://github.com/simplex-chat/simplex-chat.git $SourceDir
}
git -C $SourceDir fetch origin stable
git -C $SourceDir checkout --detach $Revision

$env:SIMPLEX_ROOT = $RootDir
$env:SIMPLEX_GHCUP = $GhcupExe
$RootUnix = (& $Bash -lc 'cygpath -u "$SIMPLEX_ROOT"').Trim()
$GhcupUnix = (& $Bash -lc 'cygpath -u "$SIMPLEX_GHCUP"').Trim()
$build = "export PATH=/ucrt64/bin:`$PATH; cd '$RootUnix/vendor/simplex-chat'; '$GhcupUnix' run --ghc $GhcVersion --cabal $CabalVersion -- cabal update && '$GhcupUnix' run --ghc $GhcVersion --cabal $CabalVersion -- scripts/desktop/build-lib-windows.sh"
& $Bash -lc $build
if ($LASTEXITCODE -ne 0) { throw "The upstream SimpleX Windows build failed." }

$BuildDir = Join-Path $SourceDir "apps\multiplatform\common\src\commonMain\cpp\desktop\libs\windows-x86_64"
$InstallDir = Join-Path $RootDir "vendor\libsimplex"
if (-not (Test-Path (Join-Path $BuildDir "libsimplex.dll"))) { throw "libsimplex.dll was not produced." }
New-Item -ItemType Directory -Force $InstallDir | Out-Null
Copy-Item (Join-Path $BuildDir "*.dll") $InstallDir -Force
Write-Host "Built $InstallDir\libsimplex.dll"
