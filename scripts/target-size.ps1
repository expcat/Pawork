# scripts/target-size.ps1
# 用途：诊断 target/ 体积分布（只读脚本，不执行任何清理）。
# 只读取目录与文件的大小信息（Get-ChildItem | Measure-Object），
# 不删除、不移动、不修改任何文件。

[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
$TargetRoot = Join-Path $RepoRoot 'target'

<#
.SYNOPSIS
    计算单个目录的总体积（递归统计所有文件，单位 MB）。
.DESCRIPTION
    目录不存在时返回 $null；只读操作，无任何副作用。
#>
function Get-DirectorySizeMb {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        return $null
    }

    $measure = Get-ChildItem -LiteralPath $Path -Recurse -File -Force -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum -ErrorAction SilentlyContinue

    $totalBytes = 0
    if ($null -ne $measure) { $totalBytes = $measure.Sum }
    if ($null -eq $totalBytes) { $totalBytes = 0 }
    return [math]::Round($totalBytes / 1MB, 1)
}

<#
.SYNOPSIS
    输出 target/ 体积诊断报告（只读，无任何副作用）。
.DESCRIPTION
    依次统计 debug、release、gates 及 debug 的 deps/incremental/build 子目录
    （各自存在才统计，缺失跳过），再输出 target/ 总大小与最大的 10 个二级子目录。
#>
function Show-TargetSizeReport {
    param(
        [Parameter(Mandatory = $true)]
        [string]$TargetRoot
    )

    $stats = @(
        @{ Name = 'target/debug';            Path = Join-Path $TargetRoot 'debug' }
        @{ Name = 'target/release';          Path = Join-Path $TargetRoot 'release' }
        @{ Name = 'target/gates';            Path = Join-Path $TargetRoot 'gates' }
        @{ Name = 'target/debug/deps';       Path = Join-Path $TargetRoot 'debug\deps' }
        @{ Name = 'target/debug/incremental';Path = Join-Path $TargetRoot 'debug\incremental' }
        @{ Name = 'target/debug/build';      Path = Join-Path $TargetRoot 'debug\build' }
    )

    Write-Host '== 各目录体积 (MB) =='
    foreach ($item in $stats) {
        $sizeMb = Get-DirectorySizeMb -Path $item.Path
        if ($null -ne $sizeMb) {
            Write-Host ("{0,-30} {1,12:N1}" -f $item.Name, $sizeMb)
        }
    }

    $totalMb = Get-DirectorySizeMb -Path $TargetRoot
    Write-Host ''
    Write-Host ("target/ 总大小：{0:N1} MB" -f $totalMb)

    Write-Host ''
    Write-Host '== 最大的 10 个二级子目录 (MB) =='
    $top10 = Get-ChildItem -LiteralPath $TargetRoot -Directory -Force -ErrorAction SilentlyContinue |
        ForEach-Object {
            [PSCustomObject]@{
                Name   = $_.Name
                SizeMb = Get-DirectorySizeMb -Path $_.FullName
            }
        } |
        Where-Object { $null -ne $_.SizeMb } |
        Sort-Object -Property SizeMb -Descending |
        Select-Object -First 10

    foreach ($item in $top10) {
        Write-Host ("{0,-42} {1,12:N1}" -f $item.Name, $item.SizeMb)
    }
}

if (-not (Test-Path -LiteralPath $TargetRoot -PathType Container)) {
    Write-Host 'target/ 不存在，跳过体积诊断（只读脚本，不做任何清理）。'
    exit 0
}

Show-TargetSizeReport -TargetRoot $TargetRoot
exit 0
