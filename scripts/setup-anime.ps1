<#
Sets up anime playback for Trela on Windows. Movies and Series don't need any
of this: they work as soon as Trela is installed.

Trela runs this itself from the About page ("Set Up Anime Playback"). To run
it by hand instead:

    powershell -ExecutionPolicy Bypass -File scripts\setup-anime.ps1

Everything goes into %LOCALAPPDATA%\Trela\tools: no admin rights, no
execution-policy change, no package manager, nothing added to PATH (Trela
looks in that folder itself). Safe to re-run; anything already there is
skipped. Delete the folder to force a clean reinstall.

  ani-cli           resolves and plays episodes (a POSIX shell script)
  Git for Windows   only if it isn't installed already. ani-cli runs under
                    its bash, sed and grep. Unpacked portable, no installer.
  fzf               ani-cli refuses to start without it, even though Trela
                    never shows its menus
  curl-impersonate  gets ani-cli and Trela past the anime site's Cloudflare
                    check

mpv isn't downloaded: Trela hands ani-cli its own bundled copy.
#>

$ErrorActionPreference = 'Stop'
# Windows PowerShell 5.1 defaults: TLS 1.0, which GitHub refuses, and a
# progress bar that makes Invoke-WebRequest many times slower.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$ProgressPreference = 'SilentlyContinue'

$Tools = Join-Path $env:LOCALAPPDATA 'Trela\tools'

function Step($message) { Write-Host "==> $message" }

function Download($url, $outFile) {
    Invoke-WebRequest -Uri $url -OutFile $outFile -UseBasicParsing
}

# Download URL of the asset in a GitHub repo's latest release whose name
# matches $pattern.
function Get-LatestAsset($repo, $pattern) {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest" -UseBasicParsing
    $asset = $release.assets | Where-Object { $_.name -match $pattern } | Select-Object -First 1
    if (-not $asset) { throw "No download matching '$pattern' in $repo's latest release ($($release.tag_name))." }
    $asset.browser_download_url
}

# Never the bare "bash" on PATH: on most machines that's WSL's
# System32\bash.exe, not Git's.
function Find-GitBash {
    $roots = @(
        (Join-Path $Tools 'git'),
        (Join-Path $env:ProgramFiles 'Git'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Git')
    )
    $git = Get-Command git.exe -ErrorAction SilentlyContinue
    if ($git) {
        # <root>/mingw64/libexec/git-core -> <root>. Also finds Scoop's Git,
        # whose PATH entry is only a shim.
        try { $roots += [IO.Path]::GetFullPath((Join-Path (& $git.Source --exec-path) '..\..\..')) } catch {}
    }
    foreach ($root in $roots) {
        $bash = Join-Path $root 'bin\bash.exe'
        if (Test-Path $bash) { return $bash }
    }
    $null
}

function Install-PortableGit {
    Step 'Downloading Git for Windows (portable, about 60 MB)...'
    $sfx = Join-Path $env:TEMP 'trela-portablegit.7z.exe'
    Download (Get-LatestAsset 'git-for-windows/git' '^PortableGit-.*-64-bit\.7z\.exe$') $sfx
    Step 'Unpacking Git...'
    # A 7-Zip self-extractor: -o<dir> unpacks there, -y skips every prompt.
    $p = Start-Process -FilePath $sfx -ArgumentList "-o`"$Tools\git`" -y" -Wait -PassThru
    Remove-Item $sfx -ErrorAction SilentlyContinue
    if ($p.ExitCode -ne 0) { throw "Unpacking Git failed (exit code $($p.ExitCode))." }
}

# Shell scripts here are read by bash, so they're written with LF line
# endings and no BOM, whatever this file was checked out with.
function Write-ShellScript($path, $content) {
    [IO.File]::WriteAllText($path, ($content -replace "`r", ''), (New-Object Text.UTF8Encoding $false))
}

try {
    New-Item -ItemType Directory -Force -Path $Tools | Out-Null

    $bash = Find-GitBash
    if ($bash) {
        Step "Git found: $bash"
    } else {
        Install-PortableGit
        $bash = Find-GitBash
        if (-not $bash) { throw "Git was unpacked but $Tools\git\bin\bash.exe is missing." }
    }

    $aniCli = Join-Path $Tools 'ani-cli'
    if (Test-Path $aniCli) {
        Step 'ani-cli already installed.'
    } else {
        Step 'Downloading ani-cli...'
        # master rather than the latest release: Trela's own title matching
        # (anidb.rs) follows whatever site master searches, and master is
        # what ani-cli's self-update pulls too.
        Download 'https://raw.githubusercontent.com/pystardust/ani-cli/master/ani-cli' $aniCli
    }

    # Rewritten every run, so it follows Git if that's been installed or
    # moved since. Same shape as the shim Scoop generates for ani-cli.
    $bashRef = if ($bash.StartsWith("$Tools\")) { '%~dp0' + $bash.Substring($Tools.Length + 1) } else { $bash }
    $oem = [Text.Encoding]::GetEncoding([Globalization.CultureInfo]::CurrentCulture.TextInfo.OEMCodePage)
    [IO.File]::WriteAllText((Join-Path $Tools 'ani-cli.cmd'), "@`"$bashRef`" `"%~dp0ani-cli`" %*`r`n", $oem)

    if (Test-Path (Join-Path $Tools 'fzf.exe')) {
        Step 'fzf already installed.'
    } else {
        Step 'Downloading fzf...'
        $zip = Join-Path $env:TEMP 'trela-fzf.zip'
        Download (Get-LatestAsset 'junegunn/fzf' '-windows_amd64\.zip$') $zip
        Expand-Archive -LiteralPath $zip -DestinationPath $Tools -Force
        Remove-Item $zip -ErrorAction SilentlyContinue
    }

    if (Test-Path (Join-Path $Tools 'curl-impersonate.exe')) {
        Step 'curl-impersonate already installed.'
    } else {
        Step 'Downloading curl-impersonate...'
        $tgz = Join-Path $env:TEMP 'trela-curl-impersonate.tar.gz'
        Download (Get-LatestAsset 'lexiforest/curl-impersonate' '^curl-impersonate-.*\.x86_64-win32\.tar\.gz$') $tgz
        # Windows' own tar, by full path: a Git tar earlier on PATH reads
        # "C:\..." as a remote host.
        & "$env:SystemRoot\System32\tar.exe" -xzf $tgz -C $Tools ./curl-impersonate.exe
        if ($LASTEXITCODE -ne 0) { throw "Unpacking curl-impersonate failed (tar exit code $LASTEXITCODE)." }
        Remove-Item $tgz -ErrorAction SilentlyContinue
    }

    # ani-cli looks for a curl_<browser> wrapper by name, and bash can't run
    # the .bat ones the download ships with. This is that package's own
    # curl_chrome116.bat, as a shell script; anidb.rs sends the same
    # fingerprint.
    Write-ShellScript (Join-Path $Tools 'curl_chrome116') @'
#!/bin/sh
exec "$(dirname "$0")/curl-impersonate.exe" \
    --ciphers "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:ECDHE-RSA-AES128-SHA:ECDHE-RSA-AES256-SHA:AES128-GCM-SHA256:AES256-GCM-SHA384:AES128-SHA:AES256-SHA" \
    -H 'sec-ch-ua: "Chromium";v="116", "Not)A;Brand";v="24", "Google Chrome";v="116"' \
    -H 'sec-ch-ua-mobile: ?0' \
    -H 'sec-ch-ua-platform: "Windows"' \
    -H 'Upgrade-Insecure-Requests: 1' \
    -H 'User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Safari/537.36' \
    -H 'Accept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7' \
    -H 'Sec-Fetch-Site: none' \
    -H 'Sec-Fetch-Mode: navigate' \
    -H 'Sec-Fetch-User: ?1' \
    -H 'Sec-Fetch-Dest: document' \
    -H 'Accept-Encoding: gzip, deflate, br' \
    -H 'Accept-Language: en-US,en;q=0.9' \
    --split-cookies \
    --http2 \
    --http2-settings "1:65536;2:0;3:1000;4:6291456;6:262144" \
    --http2-window-update 15663105 \
    --http2-stream-weight 256 \
    --http2-stream-exclusive 1 \
    --compressed \
    --tlsv1.2 --alps --tls-permute-extensions \
    --cert-compression brotli \
    --tls-grease \
    --tls-signed-cert-timestamps \
    "$@"
'@

    Step 'Checking ani-cli starts...'
    $version = & (Join-Path $Tools 'ani-cli.cmd') --version
    if ($LASTEXITCODE -ne 0) { throw "ani-cli didn't start (exit code $LASTEXITCODE)." }
    Write-Host "Anime playback is ready (ani-cli $version)."
} catch {
    Write-Host "Setup failed: $($_.Exception.Message)"
    exit 1
}
