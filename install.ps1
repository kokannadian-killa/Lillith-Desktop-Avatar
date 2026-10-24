# Lillith one-step installer for Windows 10/11 (per-user, local-only).
#
#   Double-click install.bat in the unzipped folder, or in PowerShell:
#       powershell -ExecutionPolicy Bypass -File .\install.ps1
#   Remove her again:  install.bat -Uninstall
#
# What it does (asks before anything big):
#   1. GitHub's "Download ZIP" (main.zip) has only Git LFS placeholders for her
#      art: it downloads the real files from your GitHub repo (~530 MB; the
#      repo is private, so it uses the GitHub CLI login: gh auth login).
#   2. Checks for Rust + the C++ build tools (offers winget installs).
#   3. Builds lillith.exe, lillith-settings.exe, lillith-admin.exe.
#   4. Installs to %LOCALAPPDATA%\Programs\Lillith with a Start menu entry
#      and a desktop shortcut.
#   5. Points her at your local .gguf model and llama-server.exe
#      (127.0.0.1 only). No model is ever downloaded.
#
# NOTE: written and syntax-checked on Linux; not yet run on a real Windows PC.
[CmdletBinding()]
param(
    [switch]$Yes,
    [switch]$Uninstall,
    [switch]$SkipBuild,
    [string]$Gguf = "",
    [string]$LlamaServer = "",
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\Lillith"
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$Root = $PSScriptRoot
$Repo = if ($env:LILLITH_REPO) { $env:LILLITH_REPO } else { "kokannadian-killa/Lillith-Desktop-Avatar" }
$Gen = Join-Path $Root "assets\generated"
$StartMenu = Join-Path ([Environment]::GetFolderPath("Programs")) "Lillith.lnk"
$DesktopLnk = Join-Path ([Environment]::GetFolderPath("Desktop")) "Lillith.lnk"

function Step($m) { Write-Host "`n==> $m" -ForegroundColor White }
function Ok($m)   { Write-Host "  + $m" -ForegroundColor Green }
function Warn($m) { Write-Host "  ! $m" -ForegroundColor Yellow }
function Die($m)  { Write-Host "error: $m" -ForegroundColor Red; exit 1 }
function Have($c) { [bool](Get-Command $c -ErrorAction SilentlyContinue) }
function Ask($q, $def = $true) {
    if ($Yes) { return $def }
    $hint = if ($def) { "[Y/n]" } else { "[y/N]" }
    while ($true) {
        $a = (Read-Host "  $q $hint").Trim().ToLower()
        if ($a -eq "") { return $def }
        if ($a -in "y", "yes") { return $true }
        if ($a -in "n", "no") { return $false }
    }
}
function Refresh-Path {
    $env:Path = [Environment]::GetEnvironmentVariable("Path", "Machine") + ";" +
                [Environment]::GetEnvironmentVariable("Path", "User") + ";" + "$env:USERPROFILE\.cargo\bin"
}

# -- uninstall ------------------------------------------------------
if ($Uninstall) {
    Step "Uninstalling Lillith (your models, settings and llama.cpp stay)"
    Get-Process lillith -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    foreach ($p in $StartMenu, $DesktopLnk) { if (Test-Path $p) { Remove-Item $p -Force; Ok "removed $p" } }
    if (Test-Path $InstallDir) { Remove-Item $InstallDir -Recurse -Force; Ok "removed $InstallDir" }
    Ok "settings kept in $env:APPDATA\lillith (delete that folder to wipe them)"
    exit 0
}

Write-Host "Lillith installer (Windows, per-user, local-only)" -ForegroundColor White
Write-Host "  folder: $Root"
if (-not (Test-Path (Join-Path $Root "Cargo.toml"))) { Die "keep install.ps1 in the unzipped Lillith folder" }

# -- 1. art ---------------------------------------------------------
Step "1/5  Her animation set"
$magic = "version https://git-lfs.github.com/spec/v1"
$pointers = @(Get-ChildItem $Gen -Recurse -File | Where-Object { $_.Length -lt 1024 } | Where-Object {
    (Get-Content $_.FullName -TotalCount 1 -ErrorAction SilentlyContinue) -eq $magic })
if ($pointers.Count -eq 0) {
    Ok "all real files"
} else {
    $mb = [math]::Round((($pointers | ForEach-Object {
        [long]((Get-Content $_.FullName | Where-Object { $_ -like "size *" }) -replace "size ", "") } |
        Measure-Object -Sum).Sum) / 1MB)
    Warn "$($pointers.Count) art files are Git LFS placeholders ($mb MB to download)"
    if (-not (Ask "Download her art from your GitHub repo now?")) { Die "her art is needed; re-run when ready" }
    $tok = if ($env:GH_TOKEN) { $env:GH_TOKEN } elseif ($env:GITHUB_TOKEN) { $env:GITHUB_TOKEN } else { "" }
    if (-not $tok) {
        if (-not (Have gh)) {
            if ((Have winget) -and (Ask "Install the GitHub CLI (winget install GitHub.cli) to log in?")) {
                winget install --id GitHub.cli -e --accept-source-agreements --accept-package-agreements
                Refresh-Path; $env:Path += ";$env:ProgramFiles\GitHub CLI"
            }
        }
        if (Have gh) {
            $tok = (& gh auth token --hostname github.com 2>$null)
            if (-not $tok) {
                Write-Host "  Log in to GitHub (a browser window opens):"
                & gh auth login --hostname github.com --git-protocol https --web
                $tok = (& gh auth token --hostname github.com 2>$null)
            }
        }
    }
    if (-not $tok) { Die "needs your GitHub login: install the GitHub CLI, run 'gh auth login --web', then run install.bat again" }
    $auth = "Basic " + [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes("x-access-token:$tok"))
    $items = @(foreach ($f in $pointers) {
        $t = Get-Content $f.FullName
        [pscustomobject]@{ Path = $f.FullName
            Oid = (($t | Where-Object { $_ -like "oid sha256:*" }) -replace "oid sha256:", "").Trim()
            Size = [long](($t | Where-Object { $_ -like "size *" }) -replace "size ", "") }
    })
    $done = 0
    for ($i = 0; $i -lt $items.Count; $i += 100) {
        $chunk = $items[$i..([math]::Min($i + 99, $items.Count - 1))]
        $body = @{ operation = "download"; transfers = @("basic")
                   objects = @($chunk | ForEach-Object { @{ oid = $_.Oid; size = $_.Size } }) } | ConvertTo-Json -Depth 5
        try {
            $resp = Invoke-RestMethod -Method Post -Uri "https://github.com/$Repo.git/info/lfs/objects/batch" -Body $body `
                -Headers @{ Authorization = $auth; Accept = "application/vnd.git-lfs+json" } -ContentType "application/vnd.git-lfs+json"
        } catch { Die "GitHub refused the art download ($($_.Exception.Message)). Run 'gh auth login --web' and try again." }
        $byOid = @{}; foreach ($o in $resp.objects) { $byOid[$o.oid] = $o }
        foreach ($it in $chunk) {
            $o = $byOid[$it.Oid]
            if (-not $o -or -not $o.actions.download) { Die "GitHub has no copy of $($it.Path)" }
            $h = @{}; if ($o.actions.download.header) { $o.actions.download.header.PSObject.Properties | ForEach-Object { $h[$_.Name] = $_.Value } }
            $tmp = "$($it.Path).part"
            Invoke-WebRequest -Uri $o.actions.download.href -Headers $h -OutFile $tmp -UseBasicParsing
            if ((Get-FileHash $tmp -Algorithm SHA256).Hash.ToLower() -ne $it.Oid) { Remove-Item $tmp; Die "checksum mismatch: $($it.Path)" }
            Move-Item $tmp $it.Path -Force
            $done++
        }
        Ok "$done/$($items.Count) files"
    }
    Ok "art downloaded, every file sha256-checked"
}

# -- 2. build tools -------------------------------------------------
Step "2/5  Rust and the C++ build tools"
Refresh-Path
if (-not $SkipBuild) {
    if (-not (Have cargo)) {
        if ((Have winget) -and (Ask "Rust isn't installed. Install it now (winget install Rustlang.Rustup)?")) {
            winget install --id Rustlang.Rustup -e --accept-source-agreements --accept-package-agreements
            Refresh-Path
        }
        if (-not (Have cargo)) { Die "install Rust from https://rustup.rs, then run install.bat again" }
    }
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $hasVc = (Test-Path $vswhere) -and (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
    if (-not $hasVc) {
        if ((Have winget) -and (Ask "Rust needs the Microsoft C++ Build Tools (~2 GB). Install them now?")) {
            winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-source-agreements --accept-package-agreements `
                --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
        } else { Warn "no C++ build tools found; the build may fail (install 'Build Tools for Visual Studio' with the C++ workload)" }
    }
    & rustup toolchain install stable --profile minimal 2>$null | Out-Null
    Ok "cargo $((& cargo --version) -replace 'cargo ', '')"
}

# -- 3. build -------------------------------------------------------
Step "3/5  Building (first time: 10-30 minutes)"
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root "target" }
$rel = Join-Path $target "release"
if (-not $SkipBuild) {
    Push-Location $Root
    try {
        & cargo build --release --locked -p lillith --no-default-features --features windows
        if ($LASTEXITCODE) { Die "building lillith failed (see above)" }
        & cargo build --release --locked -p lillith-settings-gui -p lillith-admin
        if ($LASTEXITCODE) { Die "building lillith-settings / lillith-admin failed (see above)" }
    } finally { Pop-Location }
}
foreach ($e in "lillith.exe", "lillith-settings.exe", "lillith-admin.exe") {
    if (-not (Test-Path (Join-Path $rel $e))) { Die "$e wasn't built in $rel" }
}
Ok "built in $rel"

# -- 4. install -----------------------------------------------------
Step "4/5  Installing to $InstallDir"
Get-Process lillith -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $InstallDir | Out-Null
foreach ($e in "lillith.exe", "lillith-settings.exe", "lillith-admin.exe") { Copy-Item (Join-Path $rel $e) $InstallDir -Force }
Copy-Item (Join-Path $Root "windows\lillith.ico") $InstallDir -Force
# Only what she reads: manifest.toml + every page/video it names.
$dstGen = Join-Path $InstallDir "assets\generated"
if (Test-Path $dstGen) { Remove-Item $dstGen -Recurse -Force }
New-Item -ItemType Directory -Force $dstGen | Out-Null
Copy-Item (Join-Path $Gen "manifest.toml") $dstGen
$refs = Select-String -Path (Join-Path $Gen "manifest.toml") -Pattern '^\s*(page|video)\s*=\s*"([^"]+)"' |
        ForEach-Object { $_.Matches[0].Groups[2].Value } | Sort-Object -Unique
foreach ($r in $refs) {
    $s = Join-Path $Gen $r
    if (Test-Path $s) { $d = Join-Path $dstGen $r; New-Item -ItemType Directory -Force (Split-Path $d) | Out-Null; Copy-Item $s $d }
}
Ok "$($refs.Count) pages and videos"
# Icon set (Settings > Icon).
$iconsToml = Get-Content (Join-Path $Root "branding\icons.toml")
foreach ($line in $iconsToml) {
    if ($line -match '^id = "(.+)"$') {
        $id = $Matches[1]; $d = Join-Path $InstallDir "icons\$id"; New-Item -ItemType Directory -Force $d | Out-Null
        foreach ($sz in 64, 128) { $p = Join-Path $Root "branding\icons\$id\lillith-$sz.png"; if (Test-Path $p) { Copy-Item $p $d } }
        $ico = Join-Path $Root "branding\icons\$id\$id.ico"; if (Test-Path $ico) { Copy-Item $ico (Join-Path $InstallDir "icons\$id.ico") }
    }
}
$sh = New-Object -ComObject WScript.Shell
foreach ($lnk in $StartMenu, $DesktopLnk) {
    if ($lnk -eq $DesktopLnk -and -not (Ask "Put a Lillith icon on the desktop?")) { continue }
    $s = $sh.CreateShortcut($lnk)
    $s.TargetPath = Join-Path $InstallDir "lillith.exe"; $s.WorkingDirectory = $InstallDir
    $s.IconLocation = Join-Path $InstallDir "lillith.ico"; $s.Description = "Lillith desktop companion"; $s.Save()
    Ok "shortcut $lnk"
}

# -- 5. local brain -------------------------------------------------
Step "5/5  Her brain (a local .gguf model; nothing is downloaded)"
if (-not $Gguf) {
    $dirs = "$env:USERPROFILE\AI\models", "$env:USERPROFILE\models", "$env:USERPROFILE\Downloads", "$env:LOCALAPPDATA\lillith\models"
    $found = @(foreach ($d in $dirs) { if (Test-Path $d) { Get-ChildItem $d -Recurse -Depth 2 -Filter *.gguf -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -notlike "*mmproj*" } } })
    $pick = $found | Sort-Object { if ($_.Name -like "dolphin3.0-llama3.2-3b-q4_k_m*") { 0 } elseif ($_.Name -like "*dolphin*") { 1 } else { 2 } } | Select-Object -First 1
    if ($pick) { $Gguf = $pick.FullName; Ok "model $Gguf" } else { Warn "no .gguf found; put one in $env:USERPROFILE\models and use Lillith Settings > Brain" }
}
if (-not $LlamaServer) {
    $cands = @("C:\llama.cpp\llama-server.exe", "$env:LOCALAPPDATA\Programs\llama.cpp\llama-server.exe", "$InstallDir\llama.cpp\llama-server.exe")
    $c = Get-Command llama-server -ErrorAction SilentlyContinue; if ($c) { $cands = @($c.Source) + $cands }
    $LlamaServer = $cands | Where-Object { Test-Path $_ } | Select-Object -First 1
    if ($LlamaServer) { Ok "llama-server $LlamaServer" } else { Warn "llama-server.exe not found; unzip a llama.cpp Windows release into C:\llama.cpp\ and use Settings > Brain" }
}
$exe = Join-Path $InstallDir "lillith.exe"
$setup = @("setup"); if ($Gguf) { $setup += "--gguf", $Gguf }; if ($LlamaServer) { $setup += "--llama-server", $LlamaServer }
& $exe @setup
if ($LASTEXITCODE) { Warn "lillith setup failed; she still runs, chat needs Settings > Brain" } else { Ok "config $env:APPDATA\lillith\config.toml (127.0.0.1 only)" }

Write-Host "`nDone. Start Lillith from the Start menu or the desktop icon." -ForegroundColor Green
Write-Host "Safeword: type 'red' (or 'OOC: stop') in her chat to stop everything."
if (-not $Yes -and (Ask "Start Lillith now?")) { Start-Process -FilePath $exe -WorkingDirectory $InstallDir }
