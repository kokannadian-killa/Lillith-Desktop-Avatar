# CI smoke for install.ps1 on Windows PowerShell 5.1 (and pwsh): parses every
# .ps1, then runs install.ps1 in throwaway folders with a fake `gh` that
# prints "no oauth token found for github.com" to stderr and exits 1 (Ko's
# crash at install.ps1:93 on 5.1). Nothing is installed: -DryRun, or -Yes
# where the art step stops cleanly before step 2.
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\windows\install-ps1-smoke.ps1
$ErrorActionPreference = "Stop"
$repo = Split-Path (Split-Path $PSScriptRoot)
$fail = 0
function Check($ok, $what) { if ($ok) { Write-Host "ok   $what" } else { Write-Host "FAIL $what"; $script:fail++ } }
Write-Host "PowerShell $($PSVersionTable.PSVersion) ($($PSVersionTable.PSEdition))"

foreach ($f in Get-ChildItem -LiteralPath $repo -Recurse -Filter *.ps1 -File | Where-Object { $_.FullName -notmatch '\\(target|\.git)\\' }) {
    $t = $null; $e = $null
    [System.Management.Automation.Language.Parser]::ParseFile($f.FullName, [ref]$t, [ref]$e) | Out-Null
    Check ($e.Count -eq 0) "parse $($f.FullName.Substring($repo.Length + 1)) $($e | ForEach-Object { $_.ToString() })"
}

$base = Join-Path ([IO.Path]::GetTempPath()) "lillith smoke $PID"
$fake = Join-Path $base "fake bin"
New-Item -ItemType Directory -Force -Path $fake | Out-Null
Set-Content -LiteralPath (Join-Path $fake "gh.cmd") -Encoding Ascii -Value "@echo no oauth token found for github.com 1>&2`r`n@exit /b 1"
$env:Path = "$fake;$env:Path"
Remove-Item Env:GH_TOKEN, Env:GITHUB_TOKEN -ErrorAction SilentlyContinue

# What 5.1 did with the old line (documents the crash being guarded against).
if ($PSVersionTable.PSVersion.Major -lt 6) {
    $threw = $false
    try { $null = (& gh auth token --hostname github.com 2>$null) } catch { $threw = $true }
    Check $threw "PS 5.1: '& gh ... 2>`$null' under Stop throws NativeCommandError (the old crash)"
}

function New-Fixture($name, [switch]$Prebuilt) {
    $d = Join-Path $base $name
    New-Item -ItemType Directory -Force -Path (Join-Path $d "assets\generated\sheets") | Out-Null
    Copy-Item -LiteralPath (Join-Path $repo "install.ps1") -Destination $d
    Copy-Item -LiteralPath (Join-Path $repo "assets\seasons.toml") -Destination (Join-Path $d "assets")
    Set-Content -LiteralPath (Join-Path $d "Cargo.toml") -Value "[workspace]"
    Set-Content -LiteralPath (Join-Path $d "assets\generated\manifest.toml") -Value 'page = "sheets/idle.png"'
    Set-Content -LiteralPath (Join-Path $d "assets\generated\sheets\idle.png") -Encoding Ascii -Value (
        "version https://git-lfs.github.com/spec/v1`noid sha256:" + ("0" * 64) + "`nsize 1048576")
    if ($Prebuilt) {
        New-Item -ItemType Directory -Force -Path (Join-Path $d "bin") | Out-Null
        Set-Content -LiteralPath (Join-Path $d "bin\lillith.exe") -Value "MZ"
    }
    $d
}
function Run-Installer($dir, [string[]]$extra) {
    $ps = (Get-Process -Id $PID).Path
    $old = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    try { $out = & $ps -NoProfile -ExecutionPolicy Bypass -File (Join-Path $dir "install.ps1") @extra 2>&1 | Out-String }
    finally { $ErrorActionPreference = $old }
    [pscustomobject]@{ Code = $LASTEXITCODE; Out = $out }
}

foreach ($kind in "source", "prebuilt") {
    $d = New-Fixture "Lillith-Desktop-Avatar-main NEWOct24 $kind\Lillith-Desktop-Avatar-main" -Prebuilt:($kind -eq "prebuilt")
    $r = Run-Installer $d @("-DryRun", "-Yes", "-InstallDir", (Join-Path $base "install $kind"))
    Write-Host $r.Out
    Check ($r.Code -eq 0) "$kind dry run exits 0 (got $($r.Code))"
    Check ($r.Out -match "Git LFS placeholders") "$kind dry run sees the LFS placeholders"
    Check ($r.Out -match "Dry run done") "$kind dry run reaches the end"
    Check ($r.Out -notmatch "NativeCommandError|no oauth token") "${kind}: gh's stderr doesn't surface or crash"
    Check (-not (Test-Path -LiteralPath (Join-Path $base "install $kind"))) "$kind dry run installs nothing"
}
# -Yes, not a dry run: no token, so it explains and stops cleanly (exit 1).
$d = New-Fixture "yes run\Lillith-Desktop-Avatar-main"
$r = Run-Installer $d @("-Yes", "-InstallDir", (Join-Path $base "install yes"))
Write-Host $r.Out
Check ($r.Code -eq 1) "-Yes without a login stops with exit 1 (got $($r.Code))"
Check ($r.Out -match "Stopped; nothing was installed") "-Yes stop message"
Check ($r.Out -match "Lillith-main-windows.zip") "points at the release zip"
Check ($r.Out -notmatch "NativeCommandError|no oauth token") "-Yes: no NativeCommandError"
Check (-not (Test-Path -LiteralPath (Join-Path $d "assets\generated\sheets\idle.png.part"))) "no .part file left behind"

Remove-Item -LiteralPath $base -Recurse -Force -ErrorAction SilentlyContinue
if ($fail) { Write-Host "$fail check(s) failed"; exit 1 }
Write-Host "install.ps1 smoke: all checks passed"
