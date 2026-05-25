# Cut a release. See VERSIONING.md for the policy.
#
# Usage:
#   .\scripts\bump-version.ps1 patch        # 0.4.0 -> 0.4.1
#   .\scripts\bump-version.ps1 minor        # 0.4.1 -> 0.5.0
#   .\scripts\bump-version.ps1 major        # 0.4.1 -> 1.0.0
#   .\scripts\bump-version.ps1 1.2.3        # explicit version
#   .\scripts\bump-version.ps1              # legacy: same as `patch`
#
# Use the `patch|minor|major` form whenever possible — the policy in
# VERSIONING.md is that the bump intent matters, not just the resulting
# number. Choosing the right slot at this script's invocation point makes
# the changelog reflect the real shape of the release.

param(
    [string]$Version = ""
)

Set-Location (Split-Path $PSScriptRoot -Parent)

# ── Resolve target version ───────────────────────────────────────────────────
$lastTag = git describe --tags --abbrev=0 2>$null
if (-not $lastTag) { $lastTag = "v0.0.0" }
$currentParts = $lastTag.TrimStart("v").Split(".")
$cMajor = [int]$currentParts[0]
$cMinor = [int]$currentParts[1]
$cPatch = [int]$currentParts[2]

switch -Regex ($Version) {
    '^patch$|^$' {
        $Version = "$cMajor.$cMinor.$($cPatch + 1)"
        Write-Host "patch bump: $lastTag -> v$Version"
    }
    '^minor$' {
        $Version = "$cMajor.$($cMinor + 1).0"
        Write-Host "minor bump: $lastTag -> v$Version"
    }
    '^major$' {
        $Version = "$($cMajor + 1).0.0"
        Write-Host "major bump: $lastTag -> v$Version"
    }
    '^\d+\.\d+\.\d+$' {
        Write-Host "explicit version: $lastTag -> v$Version"
    }
    default {
        Write-Error "Argument must be one of: patch | minor | major | <semver>. Got: $Version"
        exit 1
    }
}

$tag = "v$Version"

# Guards.
if (git tag -l $tag) {
    Write-Error "Tag $tag already exists"
    exit 1
}
$dirty = git status --porcelain
if ($dirty) {
    Write-Error "Working tree is dirty. Commit or stash changes first."
    exit 1
}

# ── Update package.json ──────────────────────────────────────────────────────
$pkgPath = "package.json"
$pkg = Get-Content $pkgPath -Raw | ConvertFrom-Json
$pkg.version = $Version
[System.IO.File]::WriteAllText(
    "$PWD\$pkgPath",
    ($pkg | ConvertTo-Json -Depth 10) + "`n",
    [System.Text.UTF8Encoding]::new($false))
Write-Host "  package.json          -> $Version"

# ── Update tauri.conf.json ───────────────────────────────────────────────────
$tauriPath = "src-tauri/tauri.conf.json"
$tauri = Get-Content $tauriPath -Raw | ConvertFrom-Json
$tauri.version = $Version
[System.IO.File]::WriteAllText(
    "$PWD\src-tauri\tauri.conf.json",
    ($tauri | ConvertTo-Json -Depth 10) + "`n",
    [System.Text.UTF8Encoding]::new($false))
Write-Host "  tauri.conf.json       -> $Version"

# ── Update Cargo.toml (first version = "..." line) ───────────────────────────
$cargoPath = "src-tauri/Cargo.toml"
$lines = Get-Content $cargoPath
$hit = $false
$lines = $lines | ForEach-Object {
    if (-not $hit -and $_ -match '^version = "') {
        $hit = $true
        "version = `"$Version`""
    } else { $_ }
}
[System.IO.File]::WriteAllLines(
    "$PWD\src-tauri\Cargo.toml",
    $lines,
    [System.Text.UTF8Encoding]::new($false))
Write-Host "  Cargo.toml            -> $Version"

# ── Commit + tag + push ──────────────────────────────────────────────────────
git add package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "chore: bump version to $Version"
git tag $tag
Write-Host ""
Write-Host "Pushing $tag to origin..."
git push origin main
git push origin $tag

Write-Host ""
Write-Host "Done. GitHub Actions will build, sign, and publish the release."
Write-Host "Track progress: https://github.com/BumStill/CodeFactory/actions"
