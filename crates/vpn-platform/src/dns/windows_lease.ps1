# The caller substitutes numeric interface/server values only. stdin is a lease:
# EOF on client crash closes the rule without depending on a Rust exit handler.
$ErrorActionPreference = 'Stop'
$mutex = [System.Threading.Mutex]::new($false, 'Global\com.xeflow.yilian.vpn.dns')
try { $acquired = $mutex.WaitOne(0) } catch [System.Threading.AbandonedMutexException] { $acquired = $true }
if (-not $acquired) { $mutex.Dispose(); throw 'Another VPN DNS lease is active' }
try {
$rule = $null
$base = $null
# cleanup has already removed stale rules; any remaining owned rule is a live lease.
if (Get-DnsClientNrptRule | Where-Object { $_.Comment -eq 'com.xeflow.yilian.vpn' }) {
    throw 'Another VPN DNS lease is still active'
}
try {
    Set-DnsClientServerAddress -InterfaceIndex __IFINDEX__ -ServerAddresses '__SERVER__'
    $rule = Add-DnsClientNrptRule -Namespace '.' -NameServers '__SERVER__' -Comment 'com.xeflow.yilian.vpn' -DisplayName ('yilian-dns-lease:{0}:{1}' -f $PID, (Get-Process -Id $PID).StartTime.ToUniversalTime().Ticks) -PassThru

    # Preserve the cmdlet-generated schema, but make only OUR rule volatile.
    # A full reboot/power loss must not resurrect a DNS server behind a dead TUN.
    $base = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig', $true)
    if ($null -eq $base) { throw 'NRPT registry key is missing' }
    $key = $base.OpenSubKey($rule.Name)
    if ($null -eq $key) { throw 'Created NRPT rule is missing' }
    $values = @()
    try {
        foreach ($name in $key.GetValueNames()) {
            $values += ,@($name, $key.GetValue($name), $key.GetValueKind($name))
        }
    } finally { $key.Dispose() }
    $base.DeleteSubKey($rule.Name)
    $key = $base.CreateSubKey($rule.Name, [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree, [Microsoft.Win32.RegistryOptions]::Volatile)
    try {
        foreach ($value in $values) { $key.SetValue($value[0], $value[1], $value[2]) }
    } finally { $key.Dispose() }
    Clear-DnsClientCache
    [Console]::Out.WriteLine('ready')
    [Console]::Out.Flush()
    [Console]::In.ReadLine() | Out-Null
} finally {
    try {
        if ($null -ne $rule) {
            # Exact identity: a late old lease cannot delete a newer connection's rule.
            if ($null -ne $base) { $base.DeleteSubKey($rule.Name, $false) }
            else { Remove-DnsClientNrptRule -Name $rule.Name -Force }
        }
    } finally {
        if ($null -ne $base) { $base.Dispose() }
        if (Get-NetAdapter -InterfaceIndex __IFINDEX__ -ErrorAction SilentlyContinue) {
            Set-DnsClientServerAddress -InterfaceIndex __IFINDEX__ -ResetServerAddresses
        }
        Clear-DnsClientCache
    }
}

} finally { $mutex.ReleaseMutex(); $mutex.Dispose() }
