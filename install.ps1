# Lillith one-step installer for Windows 10/11 (per-user, local-only).
#
#   Double-click install.bat in the unzipped folder, or in PowerShell:
#       powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1
#   Remove her again:  install.bat -Uninstall
#   Look without changing anything:  install.bat -DryRun
#
# Two kinds of folder work:
#   * The release zip (Lillith-main-windows.zip): prebuilt bin\*.exe, her
#     real art, bin\ffmpeg.exe + bin\ffprobe.exe, the offline voice (voice\)
#     and llama-server (bundle\llama\windows-x64\). Nothing is built or
#     downloaded. This is the easy way.
#   * A source checkout / GitHub "Download ZIP": builds the .exe files with
#     Rust; GitHub's ZIP has only Git LFS placeholders for her art, so it
#     offers `git lfs pull` or a download with your GitHub login (asked).
#
# Unzip to a short folder such as C:\Lillith (long paths can break a build).
# What it does (asks before anything big):
#   1. Her art (real files, or LFS placeholders fetched with your consent).
#   2/3. Uses the prebuilt .exe files, or checks Rust + C++ tools and builds.
#   4. Installs to %LOCALAPPDATA%\Programs\Lillith with a Start menu entry
#      (and, if you say so, a desktop shortcut and her voice).
#   5. Points her at your local .gguf model and llama-server.exe
#      (127.0.0.1 only). No model is ever downloaded.
[CmdletBinding()]
param(
    [switch]$Yes,
    [switch]$Uninstall,
    [switch]$SkipBuild,
    [switch]$DryRun,
    [string]$Gguf = "",
    [string]$LlamaServer = "",
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\Lillith"
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$Root = $PSScriptRoot
$Repo = if ($env:LILLITH_REPO) { $env:LILLITH_REPO } else { "kokannadian-killa/Lillith-Desktop-Avatar" }
$Releases = "https://github.com/$Repo/releases"
$Gen = Join-Path $Root "assets\generated"
$StartMenu = Join-Path ([Environment]::GetFolderPath("Programs")) "Lillith.lnk"
$DesktopLnk = Join-Path ([Environment]::GetFolderPath("Desktop")) "Lillith.lnk"
$VoiceDir = Join-Path $env:LOCALAPPDATA "Lillith\voice"
$RunKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$Exes = @("lillith.exe", "lillith-settings.exe", "lillith-admin.exe")

function Step($m) { Write-Host "`n==> $m" -ForegroundColor White }
function Ok($m)   { Write-Host "  + $m" -ForegroundColor Green }
function Warn($m) { Write-Host "  ! $m" -ForegroundColor Yellow }
function Die($m)  { Write-Host "error: $m" -ForegroundColor Red; exit 1 }
function Have($c) { [bool](Get-Command $c -ErrorAction SilentlyContinue) }
function Ask($q, $def = $true) {
    if ($Yes) { return $def }
    $hint = if ($def) { "[Y/n]" } else { "[y/N]" }
    while ($true) {
        $a = ([string](Read-Host "  $q $hint")).Trim().ToLower()
        if ($a -eq "") { return $def }
        if ($a -in "y", "yes") { return $true }
        if ($a -in "n", "no") { return $false }
    }
}
# The one way this script runs a native program (gh, git, rustup, cargo,
# lillith.exe, ...). Windows PowerShell 5.1 turns a native command's stderr
# into error records, and with ErrorActionPreference=Stop that is a
# terminating NativeCommandError even with 2>$null (e.g. gh's "no oauth token
# found"). So: Continue while it runs, never throw, report the exit code.
#   -Quiet  capture stdout (returned in .Out), drop stderr
#   default stream both to the console (builds, interactive logins)
function Run-Native {
    param([Parameter(Mandatory)][string]$Exe, [string[]]$ArgList = @(), [switch]$Quiet)
    $old = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    $code = 1; $out = @()
    try {
        if ($Quiet) { $out = @(& $Exe @ArgList 2>$null) } else { & $Exe @ArgList | Out-Host }
        $code = $LASTEXITCODE
        if ($null -eq $code) { $code = 0 }
    } catch {
        $code = 1
    } finally { $ErrorActionPreference = $old }
    [pscustomobject]@{ Code = $code; Out = (($out | ForEach-Object { [string]$_ }) -join "`n").Trim() }
}
function Refresh-Path {
    $env:Path = [Environment]::GetEnvironmentVariable("Path", "Machine") + ";" +
                [Environment]::GetEnvironmentVariable("Path", "User") + ";" + "$env:USERPROFILE\.cargo\bin"
}
function Lfs-Pointers {
    if (-not (Test-Path -LiteralPath $Gen)) { return @() }
    $magic = "version https://git-lfs.github.com/spec/v1"
    @(Get-ChildItem -LiteralPath $Gen -Recurse -File | Where-Object { $_.Length -lt 1024 } | Where-Object {
        (Get-Content -LiteralPath $_.FullName -TotalCount 1 -ErrorAction SilentlyContinue) -eq $magic })
}
function Copy-Into($src, $dstDir) {
    if ($DryRun) { Ok "(dry run) would copy $src -> $dstDir"; return }
    New-Item -ItemType Directory -Force -Path $dstDir | Out-Null
    Copy-Item -LiteralPath $src -Destination $dstDir -Force
}

# -- uninstall ------------------------------------------------------
if ($Uninstall) {
    Step "Uninstalling Lillith (your models, settings and llama.cpp stay)"
    Get-Process lillith -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    $py = Join-Path $VoiceDir "python\python.exe"
    if (Test-Path -LiteralPath $py) { Run-Native -Quiet $py @("-m", "lillith_voice.winlaunch", "--stop") | Out-Null }
    Remove-ItemProperty -Path $RunKey -Name "LillithVoice" -ErrorAction SilentlyContinue
    foreach ($p in $StartMenu, $DesktopLnk) { if (Test-Path -LiteralPath $p) { Remove-Item -LiteralPath $p -Force; Ok "removed $p" } }
    if (Test-Path -LiteralPath $InstallDir) { Remove-Item -LiteralPath $InstallDir -Recurse -Force; Ok "removed $InstallDir" }
    Ok "settings kept in $env:APPDATA\lillith, her voice in $VoiceDir (delete those folders to wipe them)"
    exit 0
}

Write-Host "Lillith installer (Windows, per-user, local-only)" -ForegroundColor White
Write-Host "  folder: $Root"
if ($DryRun) { Warn "dry run: nothing is installed, downloaded or changed" }
$Prebuilt = Test-Path -LiteralPath (Join-Path $Root "bin\lillith.exe")
if (-not $Prebuilt -and -not (Test-Path -LiteralPath (Join-Path $Root "Cargo.toml"))) {
    Die "keep install.ps1 in the unzipped Lillith folder (next to bin\ or Cargo.toml)"
}
if ($Root.Length -gt 80) {
    Warn "this folder's path is long ($($Root.Length) characters). If anything fails, unzip to C:\Lillith and run install.bat there."
}
# Files from a downloaded zip carry the "from the internet" mark, which can
# block the .exe/.dll files: clear it on this folder only.
if (-not $DryRun) {
    try { Get-ChildItem -LiteralPath $Root -Recurse -File | Unblock-File -ErrorAction SilentlyContinue; Ok "unblocked the unzipped files" }
    catch { Warn "couldn't unblock every file ($($_.Exception.Message)); right-click the zip > Properties > Unblock, then unzip again" }
}

# -- 1. art ---------------------------------------------------------
Step "1/5  Her animation set"
$NoArt = $false
$pointers = Lfs-Pointers
if ($pointers.Count -eq 0) {
    Ok "all real files"
} else {
    $mb = [math]::Round((($pointers | ForEach-Object {
        [long]((Get-Content -LiteralPath $_.FullName | Where-Object { $_ -like "size *" }) -replace "size ", "") } |
        Measure-Object -Sum).Sum) / 1MB)
    Warn "$($pointers.Count) art files are Git LFS placeholders ($mb MB to download)"
    Write-Host "  The easiest fix: download Lillith-main-windows.zip from $Releases"
    Write-Host "  (it has her real art and prebuilt programs) and run install.bat from it."
    # A git clone with git-lfs: pull them the normal way.
    if ((Test-Path -LiteralPath (Join-Path $Root ".git")) -and (Have git) -and
        ((Run-Native -Quiet git @("lfs", "version")).Code -eq 0)) {
        if ($DryRun) { Ok "(dry run) would offer: git lfs pull" }
        elseif (Ask "This is a git clone with Git LFS. Run 'git lfs pull' now?") {
            $r = Run-Native git @("-C", $Root, "lfs", "pull")
            if ($r.Code -ne 0) { Warn "git lfs pull failed (exit $($r.Code))" }
        }
        $pointers = Lfs-Pointers
    }
    if ($pointers.Count -gt 0) {
        $tok = if ($env:GH_TOKEN) { $env:GH_TOKEN } elseif ($env:GITHUB_TOKEN) { $env:GITHUB_TOKEN } else { "" }
        if (-not $tok -and (Have gh)) {
            $tok = (Run-Native -Quiet gh @("auth", "token", "--hostname", "github.com")).Out
            if (-not $tok -and -not $DryRun -and (Ask "No GitHub login found. Log in now with 'gh auth login --web' (a browser opens)?" $false)) {
                Run-Native gh @("auth", "login", "--hostname", "github.com", "--git-protocol", "https", "--web") | Out-Null
                $tok = (Run-Native -Quiet gh @("auth", "token", "--hostname", "github.com")).Out
            }
        }
        if (-not $tok -and -not $Yes -and -not $DryRun) {
            Write-Host "  Or paste a GitHub token with read access to $Repo (hidden; Enter to skip)."
            $sec = Read-Host "  token" -AsSecureString
            if ($sec -and $sec.Length -gt 0) {
                $b = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($sec)
                try { $tok = ([Runtime.InteropServices.Marshal]::PtrToStringBSTR($b)).Trim() }
                finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($b) }
            }
            $sec = $null
        }
        if (-not $tok -or $DryRun) {
            if ($DryRun) { Ok "(dry run) no download; the art step ends here" }
            else {
                Warn "no GitHub login, so her art can't be downloaded here."
                Write-Host "  To get her art, do ONE of these, then run install.bat again:"
                Write-Host "    1. Easiest: download Lillith-main-windows.zip from $Releases,"
                Write-Host "       unzip it to C:\Lillith and run install.bat there (nothing to download)."
                Write-Host "    2. Install the GitHub CLI (winget install GitHub.cli), run 'gh auth login --web'."
                Write-Host "    3. Set GH_TOKEN to a token with read access to $Repo."
            }
            if ($DryRun -or (Ask "Continue the install without her art (she won't show up until it's there)?" $false)) {
                $NoArt = $true; Warn "continuing without her art"
            } else {
                Write-Host "Stopped; nothing was installed." -ForegroundColor Yellow; exit 1
            }
        } else {
            $auth = "Basic " + [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes("x-access-token:$tok"))
            $tok = $null
            $items = @(foreach ($f in $pointers) {
                $t = Get-Content -LiteralPath $f.FullName
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
                } catch { Die "GitHub refused the art download ($($_.Exception.Message)). Run 'gh auth login --web' and try again, or use the release zip from $Releases." }
                $byOid = @{}; foreach ($o in $resp.objects) { $byOid[$o.oid] = $o }
                foreach ($it in $chunk) {
                    $o = $byOid[$it.Oid]
                    if (-not $o -or -not $o.actions.download) { Die "GitHub has no copy of $($it.Path)" }
                    $h = @{}; if ($o.actions.download.header) { $o.actions.download.header.PSObject.Properties | ForEach-Object { $h[$_.Name] = $_.Value } }
                    $tmp = "$($it.Path).part"
                    try { Invoke-WebRequest -Uri $o.actions.download.href -Headers $h -OutFile $tmp -UseBasicParsing }
                    catch {
                        if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Force }
                        Die "download failed for $($it.Path) ($($_.Exception.Message)); run install.bat again to resume ($done files already done)"
                    }
                    if ((Get-FileHash -LiteralPath $tmp -Algorithm SHA256).Hash.ToLower() -ne $it.Oid) { Remove-Item -LiteralPath $tmp -Force; Die "checksum mismatch: $($it.Path)" }
                    Move-Item -LiteralPath $tmp -Destination $it.Path -Force
                    $done++
                }
                Ok "$done/$($items.Count) files"
            }
            $auth = $null
            Ok "art downloaded, every file sha256-checked"
        }
    }
}

# -- 2./3. programs -------------------------------------------------
if ($Prebuilt) {
    Step "2-3/5  Prebuilt programs (nothing to build)"
    $rel = Join-Path $Root "bin"
} else {
    Step "2/5  Rust and the C++ build tools"
    Refresh-Path
    # A short target folder: deep paths under Downloads hit Windows' 260-character limit.
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $env:LOCALAPPDATA "lt" }
    $rel = Join-Path $target "release"
    if ($DryRun) { Ok "(dry run) would build with cargo into $target" }
    elseif (-not $SkipBuild) {
        if (-not (Have cargo)) {
            if ((Have winget) -and (Ask "Rust isn't installed. Install it now (winget install Rustlang.Rustup)?")) {
                Run-Native winget @("install", "--id", "Rustlang.Rustup", "-e", "--accept-source-agreements", "--accept-package-agreements") | Out-Null
                Refresh-Path
            }
            if (-not (Have cargo)) { Die "install Rust from https://rustup.rs, then run install.bat again (or use the release zip from ${Releases}: no build needed)" }
        }
        $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
        $hasVc = (Test-Path -LiteralPath $vswhere) -and
            ((Run-Native -Quiet $vswhere @("-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath")).Out)
        if (-not $hasVc) {
            if ((Have winget) -and (Ask "Rust needs the Microsoft C++ Build Tools (~2 GB). Install them now?")) {
                Run-Native winget @("install", "--id", "Microsoft.VisualStudio.2022.BuildTools", "-e", "--accept-source-agreements", "--accept-package-agreements",
                    "--override", "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended") | Out-Null
            } else { Warn "no C++ build tools found; the build may fail (install 'Build Tools for Visual Studio' with the C++ workload)" }
        }
        if (Have rustup) { Run-Native -Quiet rustup @("toolchain", "install", "stable", "--profile", "minimal") | Out-Null }
        Ok "cargo $((Run-Native -Quiet cargo @('--version')).Out -replace 'cargo ', '')"

        Step "3/5  Building (first time: 10-30 minutes) into $target"
        $env:CARGO_TARGET_DIR = $target
        Push-Location -LiteralPath $Root
        try {
            if ((Run-Native cargo @("build", "--release", "--locked", "-p", "lillith", "--no-default-features", "--features", "windows")).Code) { Die "building lillith failed (see above)" }
            if ((Run-Native cargo @("build", "--release", "--locked", "-p", "lillith-settings-gui", "-p", "lillith-admin")).Code) { Die "building lillith-settings / lillith-admin failed (see above)" }
        } finally { Pop-Location }
    }
}
if (-not $DryRun) {
    foreach ($e in $Exes) {
        if (-not (Test-Path -LiteralPath (Join-Path $rel $e))) { Die "$e isn't in $rel" }
    }
    Ok "programs in $rel"
}

# -- 4. install -----------------------------------------------------
Step "4/5  Installing to $InstallDir"
if (-not $DryRun) {
    Get-Process lillith -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
}
foreach ($e in $Exes) { Copy-Into (Join-Path $rel $e) $InstallDir }
Copy-Into (Join-Path $Root "windows\lillith.ico") $InstallDir
# ffmpeg/ffprobe: her MP4 poses and Pose Builder's video import. The release
# zip ships them in bin\ (Lillith looks in <install>\bin\ first).
$ffOk = $true
foreach ($t in "ffmpeg.exe", "ffprobe.exe") {
    $s = Join-Path $Root "bin\$t"
    if (Test-Path -LiteralPath $s) { Copy-Into $s (Join-Path $InstallDir "bin") } else { $ffOk = $false }
}
$ffLic = Join-Path $Root "bin\licenses"
if (Test-Path -LiteralPath $ffLic) { Get-ChildItem -LiteralPath $ffLic -File | ForEach-Object { Copy-Into $_.FullName (Join-Path $InstallDir "bin\licenses") } }
if ($ffOk) { Ok "bundled ffmpeg + ffprobe" }
elseif (Have ffmpeg) { Ok "ffmpeg from PATH" }
else { Warn "no ffmpeg/ffprobe: poses play from their PNG pages and video import is off (put ffmpeg.exe + ffprobe.exe in $InstallDir\bin)" }
# Her animation set: manifest.toml + every page/video it names, and the
# seasonal outfits file (assets\seasons.toml, read next to assets\generated).
$dstGen = Join-Path $InstallDir "assets\generated"
if (-not $NoArt) {
    if (-not $DryRun) {
        if (Test-Path -LiteralPath $dstGen) { Remove-Item -LiteralPath $dstGen -Recurse -Force }
        New-Item -ItemType Directory -Force -Path $dstGen | Out-Null
        Copy-Item -LiteralPath (Join-Path $Gen "manifest.toml") -Destination $dstGen
    }
    $refs = Select-String -LiteralPath (Join-Path $Gen "manifest.toml") -Pattern '^\s*(page|video)\s*=\s*"([^"]+)"' |
            ForEach-Object { $_.Matches[0].Groups[2].Value } | Sort-Object -Unique
    $n = 0
    foreach ($r in $refs) {
        $s = Join-Path $Gen $r
        if (Test-Path -LiteralPath $s) {
            $n++
            if (-not $DryRun) { $d = Join-Path $dstGen $r; New-Item -ItemType Directory -Force -Path (Split-Path $d) | Out-Null; Copy-Item -LiteralPath $s -Destination $d }
        }
    }
    Ok "$n of $($refs.Count) pages and videos"
} else { Warn "her art was skipped (see step 1)" }
$seasons = Join-Path $Root "assets\seasons.toml"
if (Test-Path -LiteralPath $seasons) { Copy-Into $seasons (Join-Path $InstallDir "assets"); Ok "seasonal outfits" }
# Pose Builder (Settings > Pose Builder) finds its importer in <install>\tools\.
$vp = Join-Path $Root "tools\video-pipeline"
if (Test-Path -LiteralPath $vp) {
    if (-not $DryRun) {
        $dvp = Join-Path $InstallDir "tools\video-pipeline"
        if (Test-Path -LiteralPath $dvp) { Remove-Item -LiteralPath $dvp -Recurse -Force }
        New-Item -ItemType Directory -Force -Path (Join-Path $InstallDir "tools") | Out-Null
        Copy-Item -LiteralPath $vp -Destination (Join-Path $InstallDir "tools") -Recurse -Force
    }
    Ok "Pose Builder importer (needs Python 3 with numpy, scipy, opencv-python, Pillow on PATH)"
}
# Icon set (Settings > Icon).
$iconsToml = Join-Path $Root "branding\icons.toml"
foreach ($line in $(if (Test-Path -LiteralPath $iconsToml) { Get-Content -LiteralPath $iconsToml } else { @() })) {
    if ($line -match '^id = "(.+)"$') {
        $id = $Matches[1]
        foreach ($sz in 64, 128) { $p = Join-Path $Root "branding\icons\$id\lillith-$sz.png"; if (Test-Path -LiteralPath $p) { Copy-Into $p (Join-Path $InstallDir "icons\$id") } }
        $ico = Join-Path $Root "branding\icons\$id\$id.ico"; if (Test-Path -LiteralPath $ico) { Copy-Into $ico (Join-Path $InstallDir "icons") }
    }
}
# The bundled llama-server (CPU build; 127.0.0.1 only).
$bundledLlama = Join-Path $Root "bundle\llama\windows-x64"
if (Test-Path -LiteralPath (Join-Path $bundledLlama "llama-server.exe")) {
    Get-ChildItem -LiteralPath $bundledLlama -File | ForEach-Object { Copy-Into $_.FullName (Join-Path $InstallDir "llama.cpp") }
    Ok "bundled llama-server -> $InstallDir\llama.cpp"
}
# Her voice (offline: cloned voice, fallback voice, speech recognition).
$voiceSrc = Join-Path $Root "voice"
if (Test-Path -LiteralPath (Join-Path $voiceSrc "python\pythonw.exe")) {
    if ($DryRun) { Ok "(dry run) would offer her voice -> $VoiceDir" }
    elseif (Ask "Install her voice (offline, ~1.3 GB) to $VoiceDir and start it at sign-in?") {
        $py = Join-Path $VoiceDir "python\python.exe"
        if (Test-Path -LiteralPath $py) { Run-Native -Quiet $py @("-m", "lillith_voice.winlaunch", "--stop") | Out-Null }
        foreach ($d in "python", "lib", "models", "bin", "tests", "licenses") {
            $p = Join-Path $VoiceDir $d; if (Test-Path -LiteralPath $p) { Remove-Item -LiteralPath $p -Recurse -Force }
        }
        New-Item -ItemType Directory -Force -Path $VoiceDir | Out-Null
        Copy-Item -Path (Join-Path $voiceSrc "*") -Destination $VoiceDir -Recurse -Force
        $pyw = Join-Path $VoiceDir "python\pythonw.exe"
        Set-ItemProperty -Path $RunKey -Name "LillithVoice" -Value ('"' + $pyw + '" -m lillith_voice.winlaunch')
        Start-Process -FilePath $pyw -ArgumentList @("-m", "lillith_voice.winlaunch") -WorkingDirectory $VoiceDir
        Ok "her voice is in $VoiceDir (starts at sign-in)"
    }
}
if (-not $DryRun) {
    $sh = New-Object -ComObject WScript.Shell
    foreach ($lnk in $StartMenu, $DesktopLnk) {
        if ($lnk -eq $DesktopLnk -and -not (Ask "Put a Lillith icon on the desktop?")) { continue }
        $s = $sh.CreateShortcut($lnk)
        $s.TargetPath = Join-Path $InstallDir "lillith.exe"; $s.WorkingDirectory = $InstallDir
        $s.IconLocation = Join-Path $InstallDir "lillith.ico"; $s.Description = "Lillith desktop companion"; $s.Save()
        Ok "shortcut $lnk"
    }
}

# -- 5. local brain -------------------------------------------------
Step "5/5  Her brain (a local .gguf model; nothing is downloaded)"
if (-not $Gguf) {
    $dirs = "$env:USERPROFILE\AI\models", "$env:USERPROFILE\models", "$env:USERPROFILE\Downloads", "$env:LOCALAPPDATA\lillith\models"
    $found = @(foreach ($d in $dirs) { if (Test-Path -LiteralPath $d) { Get-ChildItem -LiteralPath $d -Recurse -Depth 2 -Filter *.gguf -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -notlike "*mmproj*" } } })
    $pick = $found | Sort-Object { if ($_.Name -like "dolphin3.0-llama3.2-3b-q4_k_m*") { 0 } elseif ($_.Name -like "*dolphin*") { 1 } else { 2 } } | Select-Object -First 1
    if ($pick) { $Gguf = $pick.FullName; Ok "model $Gguf" } else { Warn "no .gguf found; put one in $env:USERPROFILE\AI\models and use Lillith Settings > Brain" }
}
if (-not $LlamaServer) {
    $cands = @("$InstallDir\llama.cpp\llama-server.exe", (Join-Path $bundledLlama "llama-server.exe"),
               "C:\llama.cpp\llama-server.exe", "$env:LOCALAPPDATA\Programs\llama.cpp\llama-server.exe")
    $c = Get-Command llama-server -ErrorAction SilentlyContinue; if ($c) { $cands += $c.Source }
    $LlamaServer = $cands | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if ($LlamaServer) { Ok "llama-server $LlamaServer" } else { Warn "llama-server.exe not found; unzip a llama.cpp Windows release into C:\llama.cpp\ and use Settings > Brain" }
}
$exe = Join-Path $InstallDir "lillith.exe"
$setup = @("setup"); if ($Gguf) { $setup += "--gguf", $Gguf }; if ($LlamaServer) { $setup += "--llama-server", $LlamaServer }
if ($DryRun) {
    Ok "(dry run) would run: `"$exe`" $($setup -join ' ')"
    Write-Host "`nDry run done: nothing was changed." -ForegroundColor Green
    exit 0
}
if ((Run-Native $exe $setup).Code) { Warn "lillith setup failed; she still runs, chat needs Settings > Brain" } else { Ok "config $env:APPDATA\lillith\config.toml (127.0.0.1 only)" }

Write-Host "`nDone. Start Lillith from the Start menu or the desktop icon." -ForegroundColor Green
Write-Host "Safeword: type 'red' (or 'OOC: stop') in her chat: she stops talking at once."
if (-not $Yes -and (Ask "Start Lillith now?")) { Start-Process -FilePath $exe -WorkingDirectory $InstallDir }
