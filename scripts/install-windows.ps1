<#
Windows installer script for TrelaPlayer

This script performs a guided setup for dependencies used by the project:
- Checks for Git, Scoop, `ani-cli`, `moviebox-tui`, and Rust toolchain
- Prompts the user for consent when making system changes (installing Git, changing execution policy to install Scoop)
- Installs `ani-cli` via Scoop
- Attempts to install `moviebox-tui` via Scoop, then via `cargo install` if Scoop doesn't provide it
- Builds the `src-tauri` Rust project and runs the resulting executable

Run from the repository root in an elevated PowerShell session when required.
#>

param(
    [switch]$Auto
)

# Environment-driven dry-run for CI validation
$DryRun = $false
if ($env:INSTALL_DRY_RUN -eq '1') { $DryRun = $true }

# Log file
function Start-Log {
    $logDir = Join-Path (Get-Location) "logs"
    if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir | Out-Null }
    $date = Get-Date -Format yyyy-MM-dd_HH-mm-ss
    $global:LogFile = Join-Path $logDir "installer-$date.log"
    "Installer started at $(Get-Date)" | Out-File -FilePath $global:LogFile -Encoding UTF8
}

function Log-Info($msg) { Write-Host $msg; $msg | Out-File -FilePath $global:LogFile -Append -Encoding UTF8 }
function Log-Warn($msg) { Write-Host $msg -ForegroundColor Yellow; $msg | Out-File -FilePath $global:LogFile -Append -Encoding UTF8 }
function Log-Error($msg) { Write-Host $msg -ForegroundColor Red; $msg | Out-File -FilePath $global:LogFile -Append -Encoding UTF8 }

function Exec-Command([ScriptBlock]$cmd) {
    if ($DryRun) { Log-Info "DRYRUN: $cmd"; return @{ ExitCode = 0; Output = "DRYRUN" } }
    try {
        $out = & $cmd 2>&1
        return @{ ExitCode = $LASTEXITCODE; Output = $out }
    } catch {
        return @{ ExitCode = 1; Output = $_ }
    }
}

function Retry-Command([ScriptBlock]$cmd, [int]$retries = 3, [int]$delaySec = 5) {
    for ($i=1; $i -le $retries; $i++) {
        $r = Exec-Command $cmd
        if ($r.ExitCode -eq 0) { return $r }
        Log-Warn "Command failed (attempt $i/$retries): $($r.Output)"
        Start-Sleep -Seconds $delaySec
    }
    return $r
}

Start-Log
Log-Info "Auto: $Auto | DryRun: $DryRun"

# Load package mapping if present
$PackagesFile = Join-Path (Get-Location) "installer\packages.json"
if (Test-Path $PackagesFile) {
    try {
        $pkgJson = Get-Content $PackagesFile -Raw | ConvertFrom-Json
        Log-Info "Loaded package mapping from $PackagesFile"
    } catch {
        Log-Warn "Failed to parse $PackagesFile: $_"
        $pkgJson = $null
    }
} else { $pkgJson = $null }

function Prompt-YesNo($message, $defaultYes = $true) {
    if ($Auto) { return $defaultYes }
    $yn = if ($defaultYes) { "[Y/n]" } else { "[y/N]" }
    while ($true) {
        Write-Host "$message $yn `" -NoNewline
        $resp = Read-Host
        if ([string]::IsNullOrWhiteSpace($resp)) { return $defaultYes }
        switch ($resp.ToLower()) {
            'y' { return $true }
            'yes' { return $true }
            'n' { return $false }
            'no' { return $false }
            default { Write-Host "Please answer 'y' or 'n'." }
        }
    }
}

function Ensure-Git() {
    if (Get-Command git -ErrorAction SilentlyContinue) {
        Log-Info "Git found."
        return $true
    }
    Log-Warn "Git not found."
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        $gitId = $pkgJson?.git?.winget_id -or 'Git.Git'
        if (Prompt-YesNo "Install Git for Windows via winget now?" $true) {
            try {
                $r = Retry-Command { winget install --id $gitId -e --silent --accept-package-agreements --accept-source-agreements }
                if ($r.ExitCode -ne 0) { throw $r.Output }
            } catch {
                Log-Warn "winget install failed: $_; opening download page."
                if (-not $DryRun) { Start-Process "https://git-scm.com/download/win" }
                return $false
            }
            if (Get-Command git -ErrorAction SilentlyContinue) { Log-Info "Git installed."; return $true }
        }
    }
    if (Prompt-YesNo "Open Git for Windows installer webpage now?" $true) {
        if (-not $DryRun) { Start-Process "https://git-scm.com/download/win" }
        Log-Warn "Please install Git for Windows and re-run this script after installation."
        return $false
    }
    return $false
}

function Ensure-Scoop() {
    if (Get-Command scoop -ErrorAction SilentlyContinue) {
        Log-Info "Scoop found."
        return $true
    }
    Log-Warn "Scoop is not installed. Scoop is the recommended Windows package manager for this installer."
    if (-not (Prompt-YesNo "Install Scoop now? (requires changing ExecutionPolicy)" $false)) {
        Log-Warn "Skipping Scoop installation."
        return $false
    }
    Log-Info "Installing Scoop (will set ExecutionPolicy for current user) ..."
    try {
        if (-not $DryRun) { Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser -Force }
        $r = Retry-Command { iex (New-Object System.Net.WebClient).DownloadString('https://get.scoop.sh') }
        if ($r.ExitCode -eq 0 -and (Get-Command scoop -ErrorAction SilentlyContinue)) {
            Log-Info "Scoop installed."; return $true
        }
    } catch {
        Log-Error "Scoop installation failed: $_"
    }
    return $false
}

function Install-AniCli() {
    if (Get-Command ani-cli -ErrorAction SilentlyContinue) {
        Log-Info "ani-cli already on PATH."
        return $true
    }
    if (-not (Get-Command scoop -ErrorAction SilentlyContinue)) {
        Log-Warn "Scoop not available; cannot use 'scoop install ani-cli'."
        return $false
    }
    Log-Info "Installing ani-cli (scoop install ani-cli) ..."
    $scoopAni = $pkgJson?.scoop?['ani-cli'] -or 'ani-cli'
    $r = Retry-Command { scoop install $scoopAni }
    if ($r.ExitCode -eq 0) { Log-Info "ani-cli installed."; return $true }
    Log-Warn "ani-cli installation failed: $($r.Output)"
    # Try winget as fallback
        if (Get-Command winget -ErrorAction SilentlyContinue) {
            Log-Info "Attempting winget install for ani-cli..."
            $wingetAni = $pkgJson?.winget?.['ani-cli'] -or 'Ani.Cli'
            $r = Retry-Command { winget install --source winget --id $wingetAni -e --silent --accept-package-agreements --accept-source-agreements }
            if ($r.ExitCode -eq 0 -and (Get-Command ani-cli -ErrorAction SilentlyContinue)) { Log-Info "ani-cli installed via winget."; return $true }
        }
    return $false
}

function Install-MovieBoxTui() {
    # Try Scoop first
    if (Get-Command moviebox-tui -ErrorAction SilentlyContinue -or Test-Path "$env:USERPROFILE\AppData\Local\MovieBox-Tui") {
        Write-Host "moviebox-tui already installed or present." -ForegroundColor Green
        return $true
    }

    if (Get-Command scoop -ErrorAction SilentlyContinue) {
        Log-Info "Attempting: scoop install moviebox-tui ..."
        $r = Retry-Command { scoop install moviebox-tui }
        if ($r.ExitCode -eq 0 -and (Get-Command moviebox-tui -ErrorAction SilentlyContinue)) { Log-Info "moviebox-tui installed via Scoop."; return $true }
        Log-Warn "Scoop install for moviebox-tui did not produce usable binary: $($r.Output)"
    }

    # Try winget next (if available)
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        Write-Host "Attempting: winget install moviebox-tui ..."
        try {
            & winget install --id "MovieBox.Tui" -e --silent --accept-source-agreements --accept-package-agreements
        } catch {
            # winget might not have an exact id; swallow and continue to cargo fallback
        }
        if (Get-Command moviebox-tui -ErrorAction SilentlyContinue) {
            Write-Host "moviebox-tui installed via winget." -ForegroundColor Green
            return $true
        }
        Write-Host "winget install attempt did not produce a usable moviebox-tui." -ForegroundColor Yellow
    }

    # Try cargo install as a fallback
    if (Get-Command cargo -ErrorAction SilentlyContinue) {
        Write-Host "Attempting: cargo install moviebox-tui --locked ..."
        & cargo install --locked --force moviebox-tui
        if ($LASTEXITCODE -eq 0 -and (Get-Command moviebox-tui -ErrorAction SilentlyContinue)) {
            Write-Host "moviebox-tui installed via cargo." -ForegroundColor Green
            return $true
        }
        Write-Host "cargo install moviebox-tui failed or binary not found on PATH." -ForegroundColor Yellow
    } else {
        Write-Host "Cargo (Rust) not found. Will attempt to install Rust next if you consent." -ForegroundColor Yellow
    }

    Log-Warn "Could not automatically install moviebox-tui. Attempting GitHub releases downloader fallback."
    if (Download-MovieBoxTuiFromReleases()) { return $true }
    Log-Warn "Automatic release download failed. Please install manually (Scoop, winget, or from releases)."
    if (Prompt-YesNo "Open moviebox-tui crate page in browser now?" $true) {
        if (-not $DryRun) { Start-Process "https://crates.io/crates/moviebox-tui" }
    }
    return $false
}

function Download-MovieBoxTuiFromReleases() {
    try {
        Log-Info "Querying crates.io for moviebox-tui metadata..."
        $meta = Invoke-RestMethod -Uri 'https://crates.io/api/v1/crates/moviebox-tui' -UseBasicParsing
        $repo = $meta.crate.repository
        if (-not $repo) { Log-Warn "Repository URL not found on crates.io"; return $false }
        Log-Info "Repository: $repo"
        if ($repo -notmatch 'github.com') { Log-Warn "Non-GitHub repository support not implemented"; return $false }
        # Normalize to API URL
        $repoPath = ($repo -replace 'https://github.com/','').TrimEnd('/').Trim()
        $apiUrl = "https://api.github.com/repos/$repoPath/releases/latest"
        Log-Info "Fetching latest release from $apiUrl"
        $headers = @{ 'User-Agent' = 'Trela-Installer' }
        $rel = Invoke-RestMethod -Uri $apiUrl -Headers $headers -UseBasicParsing
        foreach ($asset in $rel.assets) {
            if ($asset.name -match '\\.(exe|zip|msi)$' -or $asset.name -match 'windows|win') {
                Log-Info "Found candidate asset: $($asset.name) -> $($asset.browser_download_url)"
                $outFile = Join-Path $env:TEMP $asset.name
                if ($DryRun) { Log-Info "DRYRUN would download $($asset.browser_download_url) to $outFile"; return $false }
                Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $outFile
                # If zip, extract; if exe/msi, move to AppData Local
                if ($outFile -match '\\.(zip)$') {
                    $dest = Join-Path $env:LOCALAPPDATA 'MovieBox-Tui'
                    if (-not (Test-Path $dest)) { New-Item -ItemType Directory -Path $dest | Out-Null }
                    Expand-Archive -LiteralPath $outFile -DestinationPath $dest -Force
                    $bin = Get-ChildItem -Path $dest -Filter *.exe -Recurse -File | Select-Object -First 1
                    if ($bin) { Log-Info "Installed moviebox-tui from zip to $($bin.FullName)"; return $true }
                } elseif ($outFile -match '\\.(exe|msi)$') {
                    $dest = Join-Path $env:LOCALAPPDATA 'Programs\\MovieBox-Tui'
                    if (-not (Test-Path $dest)) { New-Item -ItemType Directory -Path $dest -Force | Out-Null }
                    Move-Item -Path $outFile -Destination $dest -Force
                    Log-Info "Placed release asset into $dest. You may need to add it to PATH.";
                    return $true
                }
            }
        }
        Log-Warn "No suitable release asset found in latest release."
        return $false
    } catch {
        Log-Warn "Release download attempt failed: $_"
        return $false
    }
}

function Ensure-Rust() {
    if (Get-Command cargo -ErrorAction SilentlyContinue) {
        Write-Host "Rust toolchain (cargo) found." -ForegroundColor Green
        return $true
    }
    if (-not (Prompt-YesNo "Rust not found. Install Rust toolchain (rustup-init) now?" $true)) {
        return $false
    }
        Log-Info "Downloading and running rustup-init.exe ..."
        $tmp = Join-Path $env:TEMP "rustup-init.exe"
        $r = Retry-Command { Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $tmp }
        if ($r.ExitCode -ne 0) { Log-Error "Failed to download rustup-init: $($r.Output)"; return $false }
        if ($Auto) {
            $r = Retry-Command { Start-Process -FilePath $tmp -ArgumentList "-y" -Wait }
        } else {
            $r = Retry-Command { Start-Process -FilePath $tmp -Wait }
        }
        if ($r.ExitCode -ne 0) { Log-Warn "rustup installer returned non-zero: $($r.Output)" }
        # Ensure MSVC toolchain for Tauri Windows builds
        Log-Info "Ensuring Rust toolchain stable-x86_64-pc-windows-msvc"
        $rt = Retry-Command { rustup toolchain install stable-x86_64-pc-windows-msvc }
        if ($rt.ExitCode -ne 0) { Log-Warn "Failed to install MSVC Rust toolchain: $($rt.Output)" }
        $rt2 = Retry-Command { rustup default stable-x86_64-pc-windows-msvc }
        if ($rt2.ExitCode -ne 0) { Log-Warn "Failed to set default toolchain: $($rt2.Output)" }
        # If Visual Studio Build Tools are required, attempt to ensure them
        if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
            Log-Warn "MSVC compiler (cl.exe) not found. Attempting to install Visual Studio Build Tools via winget."
            if (Get-Command winget -ErrorAction SilentlyContinue) {
                $r = Retry-Command { winget install --id Microsoft.VisualStudio.2022.BuildTools -e --silent --accept-source-agreements --accept-package-agreements }
                if ($r.ExitCode -ne 0) { Log-Warn "winget install of Build Tools failed: $($r.Output)" } else { Log-Info "Visual Studio Build Tools install attempted." }
            } else { Log-Warn "winget not available; cannot auto-install Visual Studio Build Tools." }
        }
    Log-Warn "Rust installation did not add cargo to PATH in this session; you may need to re-open the shell."
    return $false
}

function Build-And-Run-Tauri() {
    $tauriDir = Join-Path (Get-Location) "src-tauri"
    if (-not (Test-Path $tauriDir)) { Write-Host "src-tauri not found." -ForegroundColor Red; return $false }
    Push-Location $tauriDir
    Log-Info "Running: cargo build --release"
    $r = Retry-Command { cargo build --release }
    if ($r.ExitCode -ne 0) { Log-Error "cargo build failed: $($r.Output)"; Pop-Location; return $false }

    # Try to locate built executable
    $targetExe = Get-ChildItem -Path target\release -Filter *.exe -File -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $targetExe) { Write-Host "Could not find release executable." -ForegroundColor Yellow; Pop-Location; return $false }
    Write-Host "Found build: $($targetExe.FullName)" -ForegroundColor Green
    if (Prompt-YesNo "Run the built executable now?" $true) {
        if (-not $DryRun) { Start-Process -FilePath $targetExe.FullName }
    }
    Pop-Location
    return $true
}

### Main flow
Write-Host "TrelaPlayer guided installer" -ForegroundColor Cyan

if (-not (Ensure-Git)) { Write-Host "Git is required. Exiting." -ForegroundColor Red; exit 1 }

$scoopPresent = Ensure-Scoop

if ($scoopPresent) { Ensure-Rust | Out-Null }

if (-not (Install-AniCli)) {
    Write-Host "ani-cli not installed. You may retry later." -ForegroundColor Yellow
}

Install-MovieBoxTui | Out-Null

if (-not (Ensure-Rust)) {
    Write-Host "Rust toolchain required to build the Tauri backend. Install and re-run this script." -ForegroundColor Red
    exit 1
}

if (-not (Build-And-Run-Tauri)) {
    Write-Host "Failed to build or run the Tauri project. Inspect output above for errors." -ForegroundColor Red
    exit 1
}

Write-Host "Installer finished." -ForegroundColor Green
