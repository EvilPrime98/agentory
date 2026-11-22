<#
.SYNOPSIS
    Deletes local branches that are already merged into a remote integration branch.

.DESCRIPTION
    Fetches and prunes origin, then lists local branches fully merged into any of
    origin/<Base> (default: dev, staging, master). Protected branches and the
    currently checked-out branch are never touched.

    Dry-run by default; pass -Apply to delete.

    Squash-merged branches are not ancestors of the base, so `git branch --merged`
    misses them. -IncludeGone also removes branches whose remote counterpart was
    deleted (typical after a PR merge with "delete branch"), using `git branch -D`.

.EXAMPLE
    .\scripts\clean-merged-branches.ps1
    .\scripts\clean-merged-branches.ps1 -Apply
    .\scripts\clean-merged-branches.ps1 -Apply -IncludeGone
#>
[CmdletBinding()]
param(
    [switch]$Apply,
    [switch]$IncludeGone,
    [string]$Remote = 'origin',
    [string[]]$Base = @('dev', 'staging', 'master', 'main'),
    [string[]]$Protected = @('master', 'main', 'dev', 'staging')
)

$ErrorActionPreference = 'Stop'

git fetch $Remote --prune | Out-Null
if ($LASTEXITCODE -ne 0) { throw "git fetch $Remote failed" }

$current = (git rev-parse --abbrev-ref HEAD).Trim()
$skip = @($Protected) + $current

$bases = $Base | Where-Object {
    git rev-parse --verify --quiet "refs/remotes/$Remote/$_" 2>$null | Out-Null
    $LASTEXITCODE -eq 0
}
if (-not $bases) { throw "None of the base branches exist on $Remote" }

$merged = [System.Collections.Generic.HashSet[string]]::new()
foreach ($b in $bases) {
    git branch --format='%(refname:short)' --merged "$Remote/$b" |
        ForEach-Object { $merged.Add($_.Trim()) | Out-Null }
}

$gone = @()
if ($IncludeGone) {
    $gone = git for-each-ref --format='%(refname:short) %(upstream:track)' refs/heads |
        Where-Object { $_ -match '\[gone\]$' } |
        ForEach-Object { ($_ -split ' ')[0] }
}

$toDelete = $merged | Where-Object { $skip -notcontains $_ } | Sort-Object
$toForce = $gone | Where-Object { $skip -notcontains $_ -and -not $merged.Contains($_) } | Sort-Object

if (-not $toDelete -and -not $toForce) {
    Write-Host "Nothing to clean up (checked against: $($bases -join ', '))."
    return
}

$verb = if ($Apply) { 'Deleting' } else { 'Would delete' }
foreach ($name in $toDelete) {
    Write-Host "$verb (merged): $name"
    if ($Apply) { git branch -d $name }
}
foreach ($name in $toForce) {
    Write-Host "$verb (remote gone): $name"
    if ($Apply) { git branch -D $name }
}

if (-not $Apply) { Write-Host "`nDry run. Re-run with -Apply to delete." }
