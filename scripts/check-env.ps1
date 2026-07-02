[CmdletBinding()]
param(
    [switch]$BuildProbe
)

$ErrorActionPreference = 'Continue'
$ProjectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$ReportsDir = Join-Path $ProjectRoot 'reports'
New-Item -ItemType Directory -Force -Path $ReportsDir | Out-Null
$Timestamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$ReportPath = Join-Path $ReportsDir "environment-windows-$Timestamp.txt"

$script:PassCount = 0
$script:WarnCount = 0
$script:FailCount = 0

function Write-Line {
    param([string]$Text = '')
    Write-Host $Text
    Add-Content -Path $ReportPath -Value $Text -Encoding UTF8
}

function Add-Check {
    param(
        [ValidateSet('PASS', 'WARN', 'FAIL', 'INFO')]
        [string]$Level,
        [string]$Name,
        [string]$Detail
    )

    switch ($Level) {
        'PASS' { $script:PassCount++; $Color = 'Green' }
        'WARN' { $script:WarnCount++; $Color = 'Yellow' }
        'FAIL' { $script:FailCount++; $Color = 'Red' }
        default { $Color = 'Cyan' }
    }

    $Line = "[$Level] $Name - $Detail"
    Write-Host $Line -ForegroundColor $Color
    Add-Content -Path $ReportPath -Value $Line -Encoding UTF8
}

function Get-CommandOutput {
    param([string]$Command, [string[]]$Arguments = @())
    try {
        $Output = & $Command @Arguments 2>&1
        if ($LASTEXITCODE -ne 0) { return $null }
        return (($Output | Out-String).Trim())
    }
    catch {
        return $null
    }
}

function Get-MajorVersion {
    param([string]$VersionText)
    if ($VersionText -match '(\d+)\.(\d+)\.(\d+)') {
        return [int]$Matches[1]
    }
    if ($VersionText -match '(\d+)\.(\d+)') {
        return [int]$Matches[1]
    }
    return $null
}

function Invoke-ProbeCommand {
    param([string]$Name, [scriptblock]$Command)
    Write-Line ""
    Write-Line "--- $Name ---"
    try {
        & $Command 2>&1 | Tee-Object -FilePath $ReportPath -Append
        if ($LASTEXITCODE -eq 0) {
            Add-Check 'PASS' $Name '命令执行成功'
        }
        else {
            Add-Check 'FAIL' $Name "退出码 $LASTEXITCODE"
        }
    }
    catch {
        Add-Check 'FAIL' $Name $_.Exception.Message
    }
}

Write-Line "ImageDB-MVP Windows 环境检查"
Write-Line "项目目录: $ProjectRoot"
Write-Line "检查时间: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz')"
Write-Line ""

# 基础系统信息
$Os = Get-CimInstance Win32_OperatingSystem -ErrorAction SilentlyContinue
if ($Os) {
    Add-Check 'INFO' 'Windows' "$($Os.Caption) $($Os.Version) / $env:PROCESSOR_ARCHITECTURE"
}
else {
    Add-Check 'WARN' 'Windows' '无法读取系统版本'
}
Add-Check 'INFO' 'PowerShell' $PSVersionTable.PSVersion.ToString()

# 项目结构
$RequiredFiles = @(
    'package.json',
    'pnpm-workspace.yaml',
    'apps/desktop/package.json',
    'apps/desktop/src-tauri/Cargo.toml',
    'apps/desktop/src-tauri/tauri.conf.json',
    'AGENTS.md',
    'CURRENT_TASK.md'
)
$MissingFiles = @($RequiredFiles | Where-Object { -not (Test-Path (Join-Path $ProjectRoot $_)) })
if ($MissingFiles.Count -eq 0) {
    Add-Check 'PASS' '项目结构' '关键文件齐全'
}
else {
    Add-Check 'FAIL' '项目结构' ("缺失: " + ($MissingFiles -join ', '))
}

# Codex CLI
$CodexVersion = Get-CommandOutput 'codex' @('--version')
if ($CodexVersion) {
    Add-Check 'PASS' 'Codex CLI' $CodexVersion
}
else {
    Add-Check 'FAIL' 'Codex CLI' '未找到 codex 命令'
}

# Git
$GitVersion = Get-CommandOutput 'git' @('--version')
if ($GitVersion) {
    Add-Check 'PASS' 'Git' $GitVersion
    $LongPaths = Get-CommandOutput 'git' @('config', '--global', '--get', 'core.longpaths')
    if ($LongPaths -eq 'true') {
        Add-Check 'PASS' 'Git 长路径' 'core.longpaths=true'
    }
    else {
        Add-Check 'WARN' 'Git 长路径' '建议执行: git config --global core.longpaths true'
    }
    $GitUserName = Get-CommandOutput 'git' @('config', '--global', '--get', 'user.name')
    $GitUserEmail = Get-CommandOutput 'git' @('config', '--global', '--get', 'user.email')
    if ($GitUserName -and $GitUserEmail) {
        Add-Check 'PASS' 'Git 提交身份' "$GitUserName <$GitUserEmail>"
    }
    else {
        Add-Check 'WARN' 'Git 提交身份' '未完整配置 user.name / user.email'
    }
}
else {
    Add-Check 'FAIL' 'Git' '未找到 git'
}

# Node.js
$NodeVersion = Get-CommandOutput 'node' @('--version')
if ($NodeVersion) {
    $NodeMajor = Get-MajorVersion $NodeVersion
    if ($NodeMajor -ge 22 -and $NodeMajor -lt 25) {
        Add-Check 'PASS' 'Node.js' "$NodeVersion（支持范围 22/24）"
    }
    elseif ($NodeMajor -ge 22) {
        Add-Check 'WARN' 'Node.js' "$NodeVersion（建议使用 Node.js 24 LTS）"
    }
    else {
        Add-Check 'FAIL' 'Node.js' "$NodeVersion（需要 Node.js 22 或 24）"
    }
}
else {
    Add-Check 'FAIL' 'Node.js' '未找到 node'
}

$NpmVersion = Get-CommandOutput 'npm' @('--version')
if ($NpmVersion) {
    Add-Check 'PASS' 'npm' $NpmVersion
}
else {
    Add-Check 'WARN' 'npm' '未找到 npm；安装 pnpm 时可能需要'
}

# pnpm
$PnpmVersion = Get-CommandOutput 'pnpm' @('--version')
if ($PnpmVersion) {
    $PnpmMajor = Get-MajorVersion $PnpmVersion
    if ($PnpmMajor -eq 10) {
        Add-Check 'PASS' 'pnpm' "$PnpmVersion（项目使用 pnpm 10）"
    }
    else {
        Add-Check 'FAIL' 'pnpm' "$PnpmVersion（项目要求 pnpm 10）"
    }
}
else {
    Add-Check 'FAIL' 'pnpm' '未找到 pnpm 10；可执行: npm install -g pnpm@10'
}

# Rust
$RustupVersion = Get-CommandOutput 'rustup' @('--version')
if ($RustupVersion) { Add-Check 'PASS' 'rustup' (($RustupVersion -split "`n")[0]) } else { Add-Check 'FAIL' 'rustup' '未找到 rustup' }

$RustcVersion = Get-CommandOutput 'rustc' @('--version')
if ($RustcVersion) { Add-Check 'PASS' 'rustc' $RustcVersion } else { Add-Check 'FAIL' 'rustc' '未找到 rustc' }

$CargoVersion = Get-CommandOutput 'cargo' @('--version')
if ($CargoVersion) { Add-Check 'PASS' 'cargo' $CargoVersion } else { Add-Check 'FAIL' 'cargo' '未找到 cargo' }

$Toolchain = Get-CommandOutput 'rustup' @('show', 'active-toolchain')
if ($Toolchain) {
    if ($Toolchain -match 'stable' -and $Toolchain -match 'windows-msvc') {
        Add-Check 'PASS' 'Rust 工具链' $Toolchain
    }
    else {
        Add-Check 'FAIL' 'Rust 工具链' "$Toolchain；需要 stable-msvc"
    }
}

$RustfmtVersion = Get-CommandOutput 'cargo' @('fmt', '--version')
if ($RustfmtVersion) { Add-Check 'PASS' 'rustfmt' $RustfmtVersion } else { Add-Check 'FAIL' 'rustfmt' '缺失；执行: rustup component add rustfmt' }

$ClippyVersion = Get-CommandOutput 'cargo' @('clippy', '--version')
if ($ClippyVersion) { Add-Check 'PASS' 'clippy' $ClippyVersion } else { Add-Check 'FAIL' 'clippy' '缺失；执行: rustup component add clippy' }

# Visual Studio C++ Build Tools
$VsWhereCandidates = @(
    "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe",
    "$env:ProgramFiles\Microsoft Visual Studio\Installer\vswhere.exe"
) | Where-Object { $_ -and (Test-Path $_) }

if ($VsWhereCandidates.Count -gt 0) {
    $VsWhere = $VsWhereCandidates[0]
    $VsPath = (& $VsWhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null | Select-Object -First 1)
    if ($VsPath) {
        Add-Check 'PASS' 'Visual Studio C++ Build Tools' $VsPath
        $VcTools = Get-ChildItem (Join-Path $VsPath 'VC\Tools\MSVC') -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
        if ($VcTools) {
            $ClPath = Join-Path $VcTools.FullName 'bin\Hostx64\x64\cl.exe'
            $NmakePath = Join-Path $VcTools.FullName 'bin\Hostx64\x64\nmake.exe'
            if (Test-Path $ClPath) { Add-Check 'PASS' 'MSVC 编译器' $ClPath } else { Add-Check 'FAIL' 'MSVC 编译器' '未找到 cl.exe' }
            if (Test-Path $NmakePath) { Add-Check 'PASS' 'nmake' $NmakePath } else { Add-Check 'WARN' 'nmake' '未找到 nmake.exe' }
        }
    }
    else {
        Add-Check 'FAIL' 'Visual Studio C++ Build Tools' '未安装“使用 C++ 的桌面开发”组件'
    }
}
else {
    Add-Check 'FAIL' 'Visual Studio C++ Build Tools' '未找到 Visual Studio Installer / vswhere.exe'
}

$WindowsSdkRoot = "${env:ProgramFiles(x86)}\Windows Kits\10\Lib"
if (Test-Path $WindowsSdkRoot) {
    $SdkVersions = Get-ChildItem $WindowsSdkRoot -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending
    if ($SdkVersions.Count -gt 0) {
        Add-Check 'PASS' 'Windows SDK' $SdkVersions[0].Name
    }
    else {
        Add-Check 'FAIL' 'Windows SDK' '未找到 SDK 版本目录'
    }
}
else {
    Add-Check 'FAIL' 'Windows SDK' '未安装 Windows 10/11 SDK'
}

# WebView2 Runtime
$WebViewRoots = @(
    "${env:ProgramFiles(x86)}\Microsoft\EdgeWebView\Application",
    "$env:ProgramFiles\Microsoft\EdgeWebView\Application",
    "$env:LOCALAPPDATA\Microsoft\EdgeWebView\Application"
) | Where-Object { $_ -and (Test-Path $_) }

$WebViewExe = $null
foreach ($Root in $WebViewRoots) {
    $Candidate = Get-ChildItem $Root -Filter 'msedgewebview2.exe' -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($Candidate) { $WebViewExe = $Candidate.FullName; break }
}
if ($WebViewExe) {
    Add-Check 'PASS' 'WebView2 Runtime' $WebViewExe
}
else {
    Add-Check 'FAIL' 'WebView2 Runtime' '未找到 Microsoft Edge WebView2 Runtime'
}

# 磁盘空间
try {
    $DriveRoot = [System.IO.Path]::GetPathRoot($ProjectRoot)
    $Drive = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='$($DriveRoot.TrimEnd('\'))'" -ErrorAction Stop
    $FreeGb = [math]::Round($Drive.FreeSpace / 1GB, 1)
    if ($FreeGb -ge 10) {
        Add-Check 'PASS' '可用磁盘空间' "$FreeGb GB"
    }
    elseif ($FreeGb -ge 5) {
        Add-Check 'WARN' '可用磁盘空间' "$FreeGb GB；建议至少 10 GB"
    }
    else {
        Add-Check 'FAIL' '可用磁盘空间' "$FreeGb GB；不足 5 GB"
    }
}
catch {
    Add-Check 'WARN' '可用磁盘空间' '无法检测'
}

# PostgreSQL 工具是信息项，不作为开发机前置条件
$PostgresVersion = Get-CommandOutput 'postgres' @('--version')
$PsqlVersion = Get-CommandOutput 'psql' @('--version')
$PgConfig = Get-CommandOutput 'pg_config' @('--version')
if ($PostgresVersion) { Add-Check 'INFO' '系统 PostgreSQL' $PostgresVersion } else { Add-Check 'INFO' '系统 PostgreSQL' '未安装或不在 PATH；不阻塞技术探针' }
if ($PsqlVersion) { Add-Check 'INFO' 'psql' $PsqlVersion }
if ($PgConfig) { Add-Check 'INFO' 'pg_config' $PgConfig }

# 可选深度构建验证
if ($BuildProbe -and $script:FailCount -eq 0) {
    Push-Location $ProjectRoot
    try {
        Invoke-ProbeCommand 'pnpm install' { pnpm install }
        Invoke-ProbeCommand 'TypeScript typecheck' { pnpm typecheck }
        Invoke-ProbeCommand 'Frontend unit tests' { pnpm test:unit }
        Invoke-ProbeCommand 'Rust tests' { pnpm rust:test }
        Invoke-ProbeCommand 'Rust clippy' { pnpm rust:clippy }
        Invoke-ProbeCommand 'Tauri build' { pnpm build }
    }
    finally {
        Pop-Location
    }
}
elseif ($BuildProbe) {
    Add-Check 'WARN' '深度构建验证' '存在基础环境失败项，已跳过'
}

Write-Line ""
Write-Line "汇总: PASS=$script:PassCount WARN=$script:WarnCount FAIL=$script:FailCount"
Write-Line "报告: $ReportPath"

if ($script:FailCount -gt 0) {
    exit 1
}
exit 0
