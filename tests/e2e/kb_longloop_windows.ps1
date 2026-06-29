param(
  [string]$Base = "http://127.0.0.1:28630",
  [string]$Docs = "C:\attune-e2e\kb-longloop-docs",
  [string]$ReportGlob = "C:\attune-e2e\full-user-e2e-*.json",
  [int]$Loops = 10,
  [string]$OutDir = "C:\attune-e2e"
)

$ErrorActionPreference = "Stop"

$reportFile = Get-ChildItem $ReportGlob | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (!$reportFile) {
  throw "missing prior full-user-e2e report with vault token"
}

$prev = Get-Content -Raw -Encoding UTF8 $reportFile.FullName | ConvertFrom-Json
$Token = $prev.vault.token
if (!$Token) {
  $Token = $prev.vault.setup.body.token
}
if (!$Token) {
  throw "missing vault token in prior report"
}

$Headers = @{ Authorization = "Bearer $Token" }

function Invoke-AttuneJson {
  param(
    [string]$Method,
    [string]$Path,
    $Body = $null,
    [int]$Timeout = 120
  )
  $uri = "$Base$Path"
  $sw = [Diagnostics.Stopwatch]::StartNew()
  try {
    if ($null -ne $Body) {
      $json = $Body | ConvertTo-Json -Depth 20 -Compress
      $resp = Invoke-RestMethod -Method $Method -Uri $uri -Headers $Headers -ContentType "application/json; charset=utf-8" -Body $json -TimeoutSec $Timeout
    } else {
      $resp = Invoke-RestMethod -Method $Method -Uri $uri -Headers $Headers -TimeoutSec $Timeout
    }
    $sw.Stop()
    return @{ ok = $true; status = 200; ms = $sw.ElapsedMilliseconds; body = $resp }
  } catch {
    $sw.Stop()
    $status = $null
    try {
      $status = [int]$_.Exception.Response.StatusCode
    } catch {}
    return @{ ok = $false; status = $status; ms = $sw.ElapsedMilliseconds; error = $_.Exception.Message }
  }
}

$docsMeta = @(
  @{ file = "intel-windows.en.md"; title = "Benchmark Intel Windows overview"; tags = @("kb-longloop", "benchmark", "intel") },
  @{ file = "intel-windows-igpu.en.md"; title = "Benchmark Intel Windows iGPU OpenVINO"; tags = @("kb-longloop", "benchmark", "intel", "openvino") },
  @{ file = "amd-windows.en.md"; title = "Benchmark AMD Windows overview"; tags = @("kb-longloop", "benchmark", "amd") },
  @{ file = "amd-windows-igpu.en.md"; title = "Benchmark AMD Windows iGPU DirectML Vulkan"; tags = @("kb-longloop", "benchmark", "amd", "directml") },
  @{ file = "amd-windows-npu.en.md"; title = "Benchmark AMD Windows NPU VitisAI"; tags = @("kb-longloop", "benchmark", "amd", "vitisai") }
)

$queries = @(
  @{ q = "Intel DirectML OCR CER 202 OpenVINO"; expect = @("Intel", "OpenVINO", "DirectML", "CER 202") },
  @{ q = "Intel Arc iGPU OpenVINO embedding reranker latency"; expect = @("Intel", "Arc", "OpenVINO", "Reranker") },
  @{ q = "AMD Radeon 780M OCR DirectML fastest path"; expect = @("AMD", "Radeon 780M", "DirectML", "OCR") },
  @{ q = "AMD XDNA 1 NPU LLM not supported 8845H"; expect = @("XDNA 1", "LLM", "NOT SUPPORTED", "8845H") },
  @{ q = "qwen2.5 7b amd win translation fail en zh"; expect = @("qwen2.5-7b", "translation", "FAIL", "en") }
)

$out = Join-Path $OutDir ("kb-longloop-report-" + (Get-Date -Format yyyyMMdd-HHmmss) + ".json")
$report = [ordered]@{
  started = (Get-Date).ToString("o")
  base = $Base
  source_report = $reportFile.FullName
  health = $null
  ingests = @()
  loops = @()
  summary = @{}
}

$report.health = Invoke-AttuneJson GET "/api/v1/status/health" $null 60
if (-not $report.health.ok) {
  $report.summary = @{
    loops = 0
    docs = $docsMeta.Count
    ingest_ok = 0
    searches = 0
    search_ok = 0
    search_with_expected = 0
    blocked = "attune health endpoint unreachable"
    report = $out
  }
  $report | ConvertTo-Json -Depth 50 | Set-Content -Encoding UTF8 $out
  $report.summary | ConvertTo-Json -Depth 10
  exit 2
}

foreach ($d in $docsMeta) {
  $path = Join-Path $Docs $d.file
  $content = Get-Content -Raw -Encoding UTF8 $path
  $body = @{
    title = $d.title
    content = $content
    source_type = "benchmark-report"
    url = "file:///$($d.file)"
    domain = "vlm-llm-benchmark"
    tags = $d.tags
  }
  $report.ingests += @{
    file = $d.file
    bytes = [Text.Encoding]::UTF8.GetByteCount($content)
    result = (Invoke-AttuneJson POST "/api/v1/ingest" $body 180)
  }
}

for ($i = 1; $i -le $Loops; $i++) {
  $loop = [ordered]@{
    i = $i
    ai_stack = $null
    searches = @()
    repeated = $null
    items = $null
  }
  $loop.ai_stack = Invoke-AttuneJson GET "/api/v1/ai_stack" $null 60
  foreach ($q in $queries) {
    $enc = [uri]::EscapeDataString($q.q)
    $r = Invoke-AttuneJson GET "/api/v1/search?q=$enc&top_k=5" $null 120
    $text = $r | ConvertTo-Json -Depth 20 -Compress
    $matched = @()
    foreach ($e in $q.expect) {
      if ($text -like "*$e*") {
        $matched += $e
      }
    }
    $loop.searches += @{
      q = $q.q
      ok = $r.ok
      ms = $r.ms
      total = $r.body.total
      cached = $r.body.cached
      matched = $matched
      result = $r
    }
  }
  $enc2 = [uri]::EscapeDataString($queries[0].q)
  $loop.repeated = Invoke-AttuneJson GET "/api/v1/search?q=$enc2&top_k=5" $null 120
  $loop.items = Invoke-AttuneJson GET "/api/v1/items?limit=20" $null 60
  $report.loops += $loop
  Start-Sleep -Milliseconds 500
}

$allSearches = @($report.loops | ForEach-Object { $_.searches } | ForEach-Object { $_ })
$report.summary = @{
  loops = $Loops
  docs = $docsMeta.Count
  ingest_ok = @($report.ingests | Where-Object { $_.result.ok }).Count
  searches = $allSearches.Count
  search_ok = @($allSearches | Where-Object { $_.ok }).Count
  search_with_expected = @($allSearches | Where-Object { $_.matched.Count -gt 0 }).Count
  report = $out
}

$report | ConvertTo-Json -Depth 50 | Set-Content -Encoding UTF8 $out
$report.summary | ConvertTo-Json -Depth 10
