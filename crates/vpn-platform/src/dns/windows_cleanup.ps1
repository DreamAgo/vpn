$ErrorActionPreference = 'Stop'
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
    if (-not $live) { $_ | Remove-DnsClientNrptRule -Force }
}
Clear-DnsClientCache
