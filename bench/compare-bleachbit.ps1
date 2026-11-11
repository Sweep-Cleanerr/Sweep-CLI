#Requires -Version 5.1
<#
.SYNOPSIS
    Compare Sweep and BleachBit CLI preview performance.

.DESCRIPTION
    Runs only preview/dry-run commands. This script never passes --clean,
    so it does not delete files or change cleaner state.

    Each selector pair is validated against `sweep list --json` and
    `bleachbit --list-cleaners` before timing starts. Timings include
    process startup and output generation (wall clock).

.PARAMETER Selectors
    Selector specs. Plain form is shared by both tools:
        system.tmp
    Mapped form is Sweep selector = BleachBit selector(s):
        firefox.history=firefox.url_history
        browsers.chrome_cache=google_chrome.cache

.PARAMETER SweepConfig
    Optional Sweep config file passed as --config to every Sweep invocation.
    Use a copy of the config inside the workspace when the benchmark runs in
    a sandbox that cannot read the user profile. Keeps the run reproducible.

.PARAMETER IncludeBleachBit
    Also measure BleachBit. Without this switch the harness measures Sweep
    only. With it, both tools run back-to-back for every measured sample.

.PARAMETER BleachBitExtraArgs
    Extra BleachBit CLI flags, for example --no-uac on Windows.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\bench\compare-bleachbit.ps1

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\bench\compare-bleachbit.ps1 `
        -Runs 10 -Warmup 2 -Selectors system.tmp,firefox.cache
#>
[CmdletBinding()]
param(
    [string]$SweepPath = '',
    [string]$SweepConfig = '',
    [string]$BleachBitPath = '',
    [Alias('include-bleachbit')]
    [switch]$IncludeBleachBit,
    [string[]]$Selectors = @(
        'system.tmp',
        'firefox.cache',
        'firefox.cookies',
        'firefox.history=firefox.url_history',
        'firefox.session',
        'browsers.chrome_cache=google_chrome.cache',
        'browsers.chrome_history=google_chrome.history',
        'browsers.chrome_cookies=google_chrome.cookies',
        'browsers.edge_cache=microsoft_edge.cache',
        'discord.cache',
        'slack.cache'
    ),
    [int]$Runs = 5,
    [int]$Warmup = 1,
    [int]$TimeoutSec = 120,
    [string]$OutDir = '',
    [string[]]$BleachBitExtraArgs = @()
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Stable, comma-free numbers in CSV and Markdown on every locale.
[System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::InvariantCulture

if ($Runs -lt 1) { throw 'Runs must be at least 1.' }
if ($Warmup -lt 0) { throw 'Warmup cannot be negative.' }
if ($TimeoutSec -lt 1) { throw 'TimeoutSec must be at least 1.' }
if ($Selectors.Count -eq 0) { throw 'At least one selector is required.' }
if ([string]::IsNullOrWhiteSpace($OutDir)) { $OutDir = Join-Path $PSScriptRoot 'results' }

function ConvertTo-ArgumentString {
    param([string[]]$Arguments)
    $quoted = foreach ($arg in $Arguments) {
        if ($arg -match '[\s"]') {
            '"' + ($arg -replace '"', '\"') + '"'
        } else {
            $arg
        }
    }
    return ($quoted -join ' ')
}

function Invoke-CliOnce {
    param(
        [string]$FilePath,
        [string[]]$Arguments,
        [int]$TimeoutSec
    )

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $FilePath
    $psi.Arguments = ConvertTo-ArgumentString $Arguments
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    $psi.WorkingDirectory = $OutDir

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $psi
    $watch = [System.Diagnostics.Stopwatch]::StartNew()

    try {
        [void]$process.Start()
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()

        if (-not $process.WaitForExit($TimeoutSec * 1000)) {
            try { $process.Kill() } catch { }
            throw "Timed out after $TimeoutSec s: $FilePath $($psi.Arguments)"
        }

        $watch.Stop()
        [pscustomobject]@{
            ElapsedMs = [math]::Round($watch.Elapsed.TotalMilliseconds, 3)
            ExitCode  = $process.ExitCode
            Stdout    = $stdoutTask.Result
            Stderr    = $stderrTask.Result
        }
    }
    finally {
        $process.Dispose()
    }
}

function Resolve-Executable {
    param([string]$Value, [string]$Label)

    if ([string]::IsNullOrWhiteSpace($Value)) { throw "$Label path is empty." }
    if (Test-Path -LiteralPath $Value -PathType Leaf) {
        return (Resolve-Path -LiteralPath $Value).ProviderPath
    }

    $command = Get-Command $Value -ErrorAction SilentlyContinue
    if ($null -ne $command) { return $command.Source }

    throw "$Label not found: $Value"
}

function Find-Sweep {
    param([string]$ExplicitPath)

    if (-not [string]::IsNullOrWhiteSpace($ExplicitPath)) {
        return Resolve-Executable $ExplicitPath 'Sweep executable'
    }

    $root = Split-Path -Parent $PSScriptRoot
    $candidates = @(
        (Join-Path $root 'target\release\sweep.exe'),
        (Join-Path $root 'target\debug\sweep.exe'),
        (Join-Path $root 'target\release\sweep'),
        (Join-Path $root 'target\debug\sweep')
    )
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).ProviderPath
        }
    }

    throw 'Sweep executable not found. Build it first: cargo +1.88-x86_64-pc-windows-gnu build --release'
}

function Find-BleachBit {
    param([string]$ExplicitPath)

    if (-not [string]::IsNullOrWhiteSpace($ExplicitPath)) {
        return Resolve-Executable $ExplicitPath 'BleachBit executable'
    }

    $candidates = New-Object System.Collections.Generic.List[string]
    $pf86 = ${env:ProgramFiles(x86)}
    $pf = $env:ProgramFiles
    $local = $env:LOCALAPPDATA
    if ($pf86) { $candidates.Add((Join-Path $pf86 'BleachBit\bleachbit_console.exe')) }
    if ($pf) { $candidates.Add((Join-Path $pf 'BleachBit\bleachbit_console.exe')) }
    if ($local) { $candidates.Add((Join-Path $local 'Programs\BleachBit\bleachbit_console.exe')) }
    $candidates.Add('bleachbit_console.exe')
    $candidates.Add('bleachbit.exe')
    $candidates.Add('bleachbit')

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).ProviderPath
        }
        $command = Get-Command $candidate -ErrorAction SilentlyContinue
        if ($null -ne $command) { return $command.Source }
    }

    throw 'BleachBit CLI not found. Install BleachBit or pass -BleachBitPath.'
}

function ConvertFrom-SelectorSpec {
    param([string]$Spec)

    $equals = $Spec.IndexOf('=')
    if ($equals -lt 0) {
        $sweep = $Spec.Trim()
        $bleachbit = @($sweep)
    }
    else {
        $sweep = $Spec.Substring(0, $equals).Trim()
        $bleachbit = @(
            $Spec.Substring($equals + 1).Split(',') |
                ForEach-Object { $_.Trim() } |
                Where-Object { $_ }
        )
    }

    if (-not $sweep -or $bleachbit.Count -eq 0) {
        throw "Invalid selector spec: $Spec"
    }

    [pscustomobject]@{
        Sweep     = $sweep
        BleachBit = $bleachbit
        Mapping   = ($bleachbit -join ',')
    }
}

function Get-SweepSelectorSet {
    param([string]$SweepExe, [int]$TimeoutSec, [string[]]$SweepGlobalArgs = @())
    $listing = Invoke-CliOnce -FilePath $SweepExe -Arguments (@($SweepGlobalArgs) + @('list', '--json')) -TimeoutSec $TimeoutSec
    if ($listing.ExitCode -ne 0) {
        throw "sweep list --json failed (exit $($listing.ExitCode)): $($listing.Stderr.Trim())"
    }

    $definitions = $listing.Stdout | ConvertFrom-Json
    $selectors = @()
    foreach ($definition in $definitions) {
        foreach ($option in $definition.options) {
            $selectors += "$($definition.id).$($option.id)"
        }
    }
    return $selectors
}

function Get-BleachBitSelectorSet {
    param([string]$BleachBitExe, [int]$TimeoutSec)
    $listing = Invoke-CliOnce -FilePath $BleachBitExe -Arguments @('--list-cleaners') -TimeoutSec $TimeoutSec
    if ($listing.ExitCode -ne 0) {
        throw "bleachbit --list-cleaners failed (exit $($listing.ExitCode)): $($listing.Stderr.Trim())"
    }

    return @($listing.Stdout -split "\r?\n" | ForEach-Object { $_.Trim() } | Where-Object { $_ })
}

function Invoke-BenchSample {
    param(
        [pscustomobject]$Pair,
        [string]$Tool,
        [int]$Run,
        [string]$SweepExe,
        [string]$BleachBitExe,
        [string[]]$ExtraBleachBitArgs,
        [string[]]$SweepGlobalArgs = @(),
        [int]$TimeoutSec
    )

    if ($Tool -eq 'Sweep') {
        $arguments = @($SweepGlobalArgs) + @('--json', '--summary', 'preview', $Pair.Sweep)
        $exe = $SweepExe
    }
    else {
        $arguments = @('--preview') + $ExtraBleachBitArgs + $Pair.BleachBit
        $exe = $BleachBitExe
    }

    $result = Invoke-CliOnce -FilePath $exe -Arguments $arguments -TimeoutSec $TimeoutSec
    $sweepDuration = $null
    $sweepFiles = $null
    $sweepBytes = $null

    if ($Tool -eq 'Sweep' -and $result.ExitCode -eq 0 -and $result.Stdout.Trim()) {
        try {
            $document = $result.Stdout | ConvertFrom-Json
            $sweepDuration = [double]$document.duration_ms
            $sweepFiles = [long]$document.files_removed
            $sweepBytes = [long]$document.reclaimed_bytes
        }
        catch {
            $sweepDuration = $null
        }
    }

    $stderrHead = @($result.Stderr.Trim() -split "\r?\n" | Where-Object { $_ } | Select-Object -First 1)
    if ($stderrHead.Count -eq 0) { $stderrHeadText = '' } else { $stderrHeadText = [string]$stderrHead[0] }
    $outputLines = @($result.Stdout -split "\r?\n" | Where-Object { $_ }).Count

    [pscustomobject]@{
        Pair             = $Pair.Sweep
        Mapping          = $Pair.Mapping
        Run              = $Run
        Tool             = $Tool
        ElapsedMs        = $result.ElapsedMs
        ExitCode         = $result.ExitCode
        StdoutChars      = $result.Stdout.Length
        StdoutLines      = $outputLines
        StderrChars      = $result.Stderr.Length
        SweepDurationMs  = $sweepDuration
        SweepFiles       = $sweepFiles
        SweepBytes       = $sweepBytes
        StderrHead       = $stderrHeadText
    }
}

function Get-Stats {
    param([object[]]$Samples, [string]$Tool)

    $ok = @($Samples | Where-Object { $_.Tool -eq $Tool -and $_.ExitCode -eq 0 })
    if ($ok.Count -eq 0) {
        return [pscustomobject]@{ Count = 0; Median = $null; Min = $null; Max = $null }
    }

    $sorted = @($ok | ForEach-Object { $_.ElapsedMs } | Sort-Object)
    $middle = [int][math]::Floor($sorted.Count / 2)
    if (($sorted.Count % 2) -eq 0) {
        $median = ($sorted[$middle - 1] + $sorted[$middle]) / 2
    }
    else {
        $median = $sorted[$middle]
    }

    [pscustomobject]@{
        Count  = $ok.Count
        Median = [double]$median
        Min    = [double]$sorted[0]
        Max    = [double]$sorted[-1]
    }
}

function Format-Bytes {
    param($Bytes)
    if ($null -eq $Bytes) { return 'n/a' }
    $value = [double]$Bytes
    foreach ($unit in @('B', 'KiB', 'MiB', 'GiB', 'TiB')) {
        if ($value -lt 1024 -or $unit -eq 'TiB') {
            return ('{0:N1} {1}' -f $value, $unit)
        }
        $value = $value / 1024
    }
}

function Format-Ms {
    param($Milliseconds)
    if ($null -eq $Milliseconds) { return 'n/a' }
    return ('{0:N1}' -f [double]$Milliseconds)
}

function Format-Range {
    param($Stats)
    if ($null -eq $Stats.Median) { return 'n/a' }
    return ('{0:N1}-{1:N1}' -f $Stats.Min, $Stats.Max)
}

function Format-Ratio {
    param($SweepStats, $BleachBitStats)
    if ($null -eq $SweepStats.Median -or $null -eq $BleachBitStats.Median -or $SweepStats.Median -le 0) {
        return 'n/a'
    }
    return ('{0:N2}x' -f ($BleachBitStats.Median / $SweepStats.Median))
}

$sweepExe = Find-Sweep $SweepPath
$bleachBitExe = ''
if ($IncludeBleachBit) {
    $bleachBitExe = Find-BleachBit $BleachBitPath
}

$sweepGlobalArgs = @()
if (-not [string]::IsNullOrWhiteSpace($SweepConfig)) {
    if (-not (Test-Path -LiteralPath $SweepConfig -PathType Leaf)) {
        throw "Sweep config not found: $SweepConfig"
    }
    $sweepGlobalArgs = @('--config', (Resolve-Path -LiteralPath $SweepConfig).ProviderPath)
}

$sweepName = Split-Path -Leaf $sweepExe
$bleachBitName = if ($IncludeBleachBit) { Split-Path -Leaf $bleachBitExe } else { 'not requested' }
$pairs = @($Selectors | ForEach-Object { ConvertFrom-SelectorSpec $_ })

if (-not (Test-Path -LiteralPath $OutDir)) {
    [void](New-Item -ItemType Directory -Path $OutDir -Force)
}

Write-Host "Sweep:     $sweepName"
Write-Host "BleachBit: $bleachBitName"
Write-Host "Validating selectors..."

$sweepSelectors = @(Get-SweepSelectorSet -SweepExe $sweepExe -TimeoutSec $TimeoutSec -SweepGlobalArgs $sweepGlobalArgs)
$bleachBitSelectors = @()
if ($IncludeBleachBit) {
    $bleachBitSelectors = @(Get-BleachBitSelectorSet -BleachBitExe $bleachBitExe -TimeoutSec $TimeoutSec)
}

foreach ($pair in $pairs) {
    if (-not ($sweepSelectors -ccontains $pair.Sweep)) {
        throw "Sweep selector not found: $($pair.Sweep). Run 'sweep list' to see valid names."
    }
    if ($IncludeBleachBit) {
        foreach ($bleachbit in $pair.BleachBit) {
            if (-not ($bleachBitSelectors -ccontains $bleachbit)) {
                throw "BleachBit selector not found: $bleachbit. Run 'bleachbit --list-cleaners' to see valid names."
            }
        }
    }
}

$sweepVersion = 'unknown'
$bleachBitVersion = if ($IncludeBleachBit) { 'unknown' } else { 'not requested' }
try {
    $versionResult = Invoke-CliOnce -FilePath $sweepExe -Arguments @('--version') -TimeoutSec $TimeoutSec
    if ($versionResult.ExitCode -eq 0) { $sweepVersion = $versionResult.Stdout.Trim() }
}
catch { }
try {
    if ($IncludeBleachBit) {
        $versionResult = Invoke-CliOnce -FilePath $bleachBitExe -Arguments @('--version') -TimeoutSec $TimeoutSec
        if ($versionResult.ExitCode -eq 0) {
            $firstLine = @($versionResult.Stdout -split "\r?\n" | Where-Object { $_.Trim() } | Select-Object -First 1)
            if ($firstLine.Count -gt 0) { $bleachBitVersion = $firstLine[0].Trim() }
        }
    }
}
catch { }

$machine = [pscustomobject]@{ CPU = 'unknown'; OS = [Environment]::OSVersion.VersionString; RAM = 'unknown' }
try {
    if ($env:OS -eq 'Windows_NT') {
        $cpu = Get-CimInstance Win32_Processor -ErrorAction Stop | Select-Object -First 1
        if ($null -ne $cpu) { $machine.CPU = $cpu.Name.Trim() }
        $os = Get-CimInstance Win32_OperatingSystem -ErrorAction Stop
        $machine.OS = ("{0} {1}" -f $os.Caption, $os.Version).Trim()
        if ($os.TotalVisibleMemorySize) {
            $machine.RAM = Format-Bytes ([long]$os.TotalVisibleMemorySize * 1024)
        }
    }
}
catch { }

$samples = New-Object System.Collections.Generic.List[object]
$summaries = @()
$toolCount = if ($IncludeBleachBit) { 2 } else { 1 }
$total = $pairs.Count * $Runs * $toolCount
$pairIndex = 0

foreach ($pair in $pairs) {
    $pairIndex++
    Write-Host "Benchmarking $($pair.Sweep) ($($pair.Mapping))..."

    if ($IncludeBleachBit) {
        $warmTools = @('Sweep', 'BleachBit')
        $measuredOrder = @('Sweep', 'BleachBit')
    }
    else {
        $warmTools = @('Sweep')
        $measuredOrder = @('Sweep')
    }

    for ($warm = 1; $warm -le $Warmup; $warm++) {
        foreach ($tool in $warmTools) {
            $null = Invoke-BenchSample -Pair $pair -Tool $tool -Run 0 `
                -SweepExe $sweepExe -BleachBitExe $bleachBitExe `
                -ExtraBleachBitArgs $BleachBitExtraArgs -SweepGlobalArgs $sweepGlobalArgs -TimeoutSec $TimeoutSec
        }
    }

    $pairSamples = @()
    for ($run = 1; $run -le $Runs; $run++) {
        if ($IncludeBleachBit -and (($run + $pairIndex) % 2) -eq 0) {
            $order = @('BleachBit', 'Sweep')
        }
        else {
            $order = $measuredOrder
        }

        foreach ($tool in $order) {
            $sample = Invoke-BenchSample -Pair $pair -Tool $tool -Run $run `
                -SweepExe $sweepExe -BleachBitExe $bleachBitExe `
                -ExtraBleachBitArgs $BleachBitExtraArgs -SweepGlobalArgs $sweepGlobalArgs -TimeoutSec $TimeoutSec
            $samples.Add($sample)
            $pairSamples += $sample
        }
    }

    $sweepSamples = @($pairSamples | Where-Object { $_.Tool -eq 'Sweep' })
    $bleachBitSamples = @($pairSamples | Where-Object { $_.Tool -eq 'BleachBit' })
    $sweepStats = Get-Stats -Samples $sweepSamples -Tool 'Sweep'
    $bleachBitStats = Get-Stats -Samples $bleachBitSamples -Tool 'BleachBit'
    $countSample = @($sweepSamples | Where-Object { $_.ExitCode -eq 0 -and $null -ne $_.SweepFiles } | Select-Object -First 1)
    $lineSample = @($bleachBitSamples | Where-Object { $_.ExitCode -eq 0 } | Select-Object -First 1)

    if ($countSample.Count -gt 0) {
        $sweepFiles = $countSample[0].SweepFiles
        $sweepBytes = $countSample[0].SweepBytes
    }
    else {
        $sweepFiles = $null
        $sweepBytes = $null
    }
    if ($lineSample.Count -gt 0) {
        $bleachBitLines = $lineSample[0].StdoutLines
    }
    else {
        $bleachBitLines = $null
    }

    $summaries += [pscustomobject]@{
        Pair             = $pair.Sweep
        Mapping          = $pair.Mapping
        Sweep            = $sweepStats
        BleachBit        = $bleachBitStats
        Ratio            = (Format-Ratio -SweepStats $sweepStats -BleachBitStats $bleachBitStats)
        SweepFiles       = $sweepFiles
        SweepBytes       = $sweepBytes
        BleachBitLines   = $bleachBitLines
        SweepFailures    = @($sweepSamples | Where-Object { $_.ExitCode -ne 0 }).Count
        BleachBitFailures = @($bleachBitSamples | Where-Object { $_.ExitCode -ne 0 }).Count
    }
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$csvName = "compare-bleachbit-$stamp.csv"
$markdownName = "compare-bleachbit-$stamp.md"
$csvPath = Join-Path $OutDir $csvName
$markdownPath = Join-Path $OutDir $markdownName

# A long benchmark must not lose every sample because the output directory
# became unwritable (sandbox, AV scan, or a moved folder). Fall back to TEMP.
function Use-FallbackOutDir {
    $fallback = Join-Path ([System.IO.Path]::GetTempPath()) "sweep-bench-$stamp"
    [void](New-Item -ItemType Directory -Path $fallback -Force)
    return $fallback
}

try {
    $samples | Export-Csv -LiteralPath $csvPath -NoTypeInformation -Encoding UTF8
}
catch {
    $fallback = Use-FallbackOutDir
    Write-Warning "Could not write CSV to $csvPath ($($_.Exception.Message)); using $fallback"
    $csvPath = Join-Path $fallback $csvName
    $markdownPath = Join-Path $fallback $markdownName
    $samples | Export-Csv -LiteralPath $csvPath -NoTypeInformation -Encoding UTF8
}

$markdown = New-Object System.Collections.Generic.List[string]
$markdown.Add('# Sweep vs BleachBit CLI benchmark')
$markdown.Add('')
$markdown.Add("- Date: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')")
$markdown.Add("- Sweep: ``$sweepVersion`` (``$sweepName``)")
$markdown.Add("- BleachBit: ``$bleachBitVersion`` (``$bleachBitName``)")
$markdown.Add("- CPU: $($machine.CPU)")
$markdown.Add("- OS: $($machine.OS)")
$markdown.Add("- RAM: $($machine.RAM)")
$markdown.Add("- PowerShell: $($PSVersionTable.PSVersion)")
$markdown.Add("- Runs per tool: $Runs (warmup: $Warmup)")
# Keep absolute paths (which contain usernames) out of the generated report.
$reportSweepArgs = @()
if ($sweepGlobalArgs.Count -gt 0) {
    $reportSweepArgs = @('--config', (Split-Path -Leaf $sweepGlobalArgs[1]))
}
$sweepCommand = 'sweep ' + (($reportSweepArgs + @('--json', '--summary', 'preview', '<selector>')) -join ' ')
if ($IncludeBleachBit) {
    $markdown.Add("- Commands: ``$sweepCommand`` vs ``bleachbit --preview <selector>``")
}
else {
    $markdown.Add("- Commands: ``$sweepCommand``")
}
$markdown.Add('- Wall clock includes process startup and output generation. Medians use successful runs only.')
if ($IncludeBleachBit) {
    $markdown.Add('- `BB/Sweep` above 1.00x means Sweep was faster. Byte counts are Sweep-only context.')
    $markdown.Add('- `BB lines` is BleachBit preview output lines; compare it with `Sweep files` to spot semantic mismatches.')
}
$markdown.Add('- Preview only: no files were deleted.')
$markdown.Add('')
$markdown.Add('| Sweep selector | BleachBit selector(s) | Sweep median (ms) | Sweep min-max | BleachBit median (ms) | BleachBit min-max | BB/Sweep | Sweep files | BB lines | Sweep bytes | Failures (S/B) |')
$markdown.Add('| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |')

foreach ($summary in $summaries) {
    $line = '| {0} | {1} | {2} | {3} | {4} | {5} | {6} | {7} | {8} | {9} | {10}/{11} |' -f `
        $summary.Pair,
        $summary.Mapping,
        (Format-Ms $summary.Sweep.Median),
        (Format-Range $summary.Sweep),
        (Format-Ms $summary.BleachBit.Median),
        (Format-Range $summary.BleachBit),
        $summary.Ratio,
        $(if ($null -eq $summary.SweepFiles) { 'n/a' } else { '{0:N0}' -f $summary.SweepFiles }),
        $(if ($null -eq $summary.BleachBitLines) { 'n/a' } else { '{0:N0}' -f $summary.BleachBitLines }),
        (Format-Bytes $summary.SweepBytes),
        $summary.SweepFailures,
        $summary.BleachBitFailures
    $markdown.Add($line)
}

$markdown.Add('')
$markdown.Add('Raw samples: `' + (Split-Path -Leaf $csvPath) + '`')

try {
    $markdown | Set-Content -LiteralPath $markdownPath -Encoding UTF8
}
catch {
    $fallback = Use-FallbackOutDir
    Write-Warning "Could not write Markdown to $markdownPath ($($_.Exception.Message)); using $fallback"
    $markdownPath = Join-Path $fallback $markdownName
    $markdown | Set-Content -LiteralPath $markdownPath -Encoding UTF8
}

$rows = foreach ($summary in $summaries) {
    [pscustomobject]@{
        Selector      = $summary.Pair
        'Sweep ms'    = Format-Ms $summary.Sweep.Median
        'Sweep range' = Format-Range $summary.Sweep
        'BB ms'       = Format-Ms $summary.BleachBit.Median
        'BB range'    = Format-Range $summary.BleachBit
        'BB/Sweep'    = $summary.Ratio
        'Sweep files' = $(if ($null -eq $summary.SweepFiles) { 'n/a' } else { '{0:N0}' -f $summary.SweepFiles })
        'BB lines'    = $(if ($null -eq $summary.BleachBitLines) { 'n/a' } else { '{0:N0}' -f $summary.BleachBitLines })
        'Sweep bytes' = Format-Bytes $summary.SweepBytes
        Failures      = "$($summary.SweepFailures)/$($summary.BleachBitFailures)"
    }
}

Write-Host ''
Write-Host ("Samples: {0} planned, {1} recorded" -f $total, $samples.Count)
$rows | Format-Table -AutoSize | Out-String -Width 4096 | Write-Host
Write-Host "CSV:      $csvPath"
Write-Host "Markdown: $markdownPath"
Write-Host 'Preview only; nothing was deleted.'
