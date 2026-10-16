# Checkup completo do PC (só leitura, sem admin). Cada seção é independente:
# se uma falhar, as outras continuam. Saída: um JSON na última linha.
$ErrorActionPreference = 'SilentlyContinue'
$r = [ordered]@{}

function Safe([scriptblock]$b) { try { & $b } catch { $null } }

# Windows
$r.os = Safe {
  $os = Get-CimInstance Win32_OperatingSystem
  $last = (Get-HotFix | Where-Object InstalledOn | Sort-Object InstalledOn -Descending | Select-Object -First 1).InstalledOn
  $pending = (Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending') -or
             (Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired')
  [ordered]@{
    name = $os.Caption; version = $os.Version; build = $os.BuildNumber
    uptime_hours = [math]::Round(((Get-Date) - $os.LastBootUpTime).TotalHours, 1)
    last_update_days = if ($last) { [int]((Get-Date) - $last).TotalDays } else { $null }
    reboot_pending = [bool]$pending
  }
}

# Processador
$r.cpu = Safe {
  $c = Get-CimInstance Win32_Processor | Select-Object -First 1
  [ordered]@{ name = $c.Name.Trim(); cores = $c.NumberOfCores; threads = $c.NumberOfLogicalProcessors
    max_mhz = $c.MaxClockSpeed; current_mhz = $c.CurrentClockSpeed; load = $c.LoadPercentage }
}

# Memória (pentes e slots)
$r.memory = Safe {
  $mods = @(Get-CimInstance Win32_PhysicalMemory)
  $slots = (Get-CimInstance Win32_PhysicalMemoryArray | Measure-Object MemoryDevices -Sum).Sum
  [ordered]@{
    modules = $mods.Count; slots = $slots
    total_gb = [math]::Round((($mods | Measure-Object Capacity -Sum).Sum) / 1GB, 1)
    rated_mhz = ($mods | Measure-Object Speed -Maximum).Maximum
    configured_mhz = ($mods | Measure-Object ConfiguredClockSpeed -Minimum).Minimum
  }
}

# Placas de vídeo (driver)
$r.gpus = @(Safe {
  Get-CimInstance Win32_VideoController | Where-Object { $_.Name -notmatch 'Basic|Remote|Virtual|Parsec|Meta' } | ForEach-Object {
    [ordered]@{ name = $_.Name; driver = $_.DriverVersion
      driver_age_days = if ($_.DriverDate) { [int]((Get-Date) - $_.DriverDate).TotalDays } else { $null } }
  }
})

# Discos físicos (saúde)
$r.disks = @(Safe {
  Get-PhysicalDisk | ForEach-Object {
    [ordered]@{ name = $_.FriendlyName; media = "$($_.MediaType)"; bus = "$($_.BusType)"; health = "$($_.HealthStatus)"
      status = "$($_.OperationalStatus)"; size_gb = [math]::Round($_.Size / 1GB) }
  }
})
$r.system_drive_media = Safe {
  $n = (Get-Partition -DriveLetter ($env:SystemDrive.TrimEnd(':')) | Get-Disk).Number
  "$((Get-PhysicalDisk | Where-Object DeviceId -eq "$n").MediaType)"
}

# Programas que iniciam com o Windows
$r.startup = @(Safe { Get-CimInstance Win32_StartupCommand | ForEach-Object { $_.Name } } | Sort-Object -Unique)

# Energia
$r.power_plan = Safe { ((powercfg /getactivescheme) -replace '^.*\((.*)\).*$', '$1').Trim() }
$r.battery = Safe {
  $full = (Get-CimInstance -Namespace root\wmi -ClassName BatteryFullChargedCapacity | Select-Object -First 1).FullChargedCapacity
  $design = (Get-CimInstance -Namespace root\wmi -ClassName BatteryStaticData | Select-Object -First 1).DesignedCapacity
  if ($full -and $design) { [ordered]@{ health_pct = [math]::Round($full * 100 / $design) } } else { $null }
}

# Segurança
$r.antivirus = Safe {
  $mp = Get-MpComputerStatus
  $others = @(Get-CimInstance -Namespace root\SecurityCenter2 -ClassName AntiVirusProduct | ForEach-Object displayName)
  [ordered]@{ defender_enabled = [bool]$mp.AntivirusEnabled; realtime = [bool]$mp.RealTimeProtectionEnabled
    signature_age_days = $mp.AntivirusSignatureAge; products = $others }
}

# Estabilidade: eventos dos últimos 7 dias (só contagens)
$r.events = Safe {
  $since = (Get-Date).AddDays(-7)
  $count = { param($filter) @(Get-WinEvent -FilterHashtable $filter -MaxEvents 200).Count }
  [ordered]@{
    unexpected_shutdowns = & $count @{ LogName = 'System'; Id = 41; ProviderName = 'Microsoft-Windows-Kernel-Power'; StartTime = $since }
    bluescreens = & $count @{ LogName = 'System'; Id = 1001; ProviderName = 'Microsoft-Windows-WER-SystemErrorReporting'; StartTime = $since }
    hardware_errors = & $count @{ LogName = 'System'; ProviderName = 'Microsoft-Windows-WHEA-Logger'; StartTime = $since; Level = 2, 3 }
    disk_errors = & $count @{ LogName = 'System'; ProviderName = 'disk'; Id = 7, 51, 153; StartTime = $since }
    app_crashes = & $count @{ LogName = 'Application'; ProviderName = 'Application Error'; Id = 1000; StartTime = $since }
    top_crashes = @(Get-WinEvent -FilterHashtable @{ LogName = 'Application'; ProviderName = 'Application Error'; Id = 1000; StartTime = $since } -MaxEvents 200 |
      Group-Object { $_.Properties[0].Value } | Sort-Object Count -Descending | Select-Object -First 3 |
      ForEach-Object { [ordered]@{ app = $_.Name; count = $_.Count } })
  }
}

# Rede
$r.network = Safe {
  $ping = @(Test-Connection 1.1.1.1 -Count 4 -ErrorAction SilentlyContinue)
  $ok = @($ping | Where-Object { $_.StatusCode -eq 0 -or $_.Status -eq 'Success' })
  $lat = ($ok | ForEach-Object { if ($_.ResponseTime -ne $null) { $_.ResponseTime } else { $_.Latency } } | Measure-Object -Average).Average
  $dns = Measure-Command { Resolve-DnsName github.com -ErrorAction SilentlyContinue | Out-Null }
  $wifi = (netsh wlan show interfaces) -match '^\s*(Sinal|Signal)\s*:' | Select-Object -First 1
  [ordered]@{ loss_pct = if ($ping.Count) { [int](100 - $ok.Count * 100 / 4) } else { 100 }
    latency_ms = if ($lat) { [math]::Round($lat) } else { $null }
    dns_ms = [math]::Round($dns.TotalMilliseconds)
    wifi_signal = if ($wifi) { [int](($wifi -replace '[^0-9]', '')) } else { $null } }
}

# Pasta temporária
$r.temp_gb = Safe { [math]::Round(((Get-ChildItem $env:TEMP -Recurse -File -Force | Measure-Object Length -Sum).Sum) / 1GB, 2) }

$r | ConvertTo-Json -Depth 5 -Compress
