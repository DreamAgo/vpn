$ErrorActionPreference = 'Stop'
# The common path is a new TUN with no local NRPT rules. Avoid starting the
# DNS CIM provider (which can be slow even though there is nothing to clean).
$base = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig')
if ($null -eq $base) { return }
try { $hasRules = $base.SubKeyCount -gt 0 } finally { $base.Dispose() }
if (-not $hasRules) { return }
$removed = $false
Get-DnsClientNrptRule | Where-Object { $_.Comment -eq 'com.xeflow.yilian.vpn' } | ForEach-Object {
    $live = $false
    if ($_.DisplayName -match '^yilian-dns-lease:([0-9]+):([0-9]+)$') {
        try {
            $ownerProcess = Get-Process -Id ([int]$Matches[1]) -ErrorAction Stop
            $live = $ownerProcess.StartTime.ToUniversalTime().Ticks -eq ([long]$Matches[2])
        } catch [Microsoft.PowerShell.Commands.ProcessCommandException] {
            # No process with this identity: the rule is stale.
        }
    }
    if (-not $live) { $_ | Remove-DnsClientNrptRule -Force; $removed = $true }
}
if ($removed) { Clear-DnsClientCache }
