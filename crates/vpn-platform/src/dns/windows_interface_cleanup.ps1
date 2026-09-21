$ErrorActionPreference = 'Stop'
# Use the local adapter snapshot instead of loading the NetAdapter CIM provider.
# A fresh TUN has no DNS addresses: resetting it would only add a slow no-op.
foreach ($adapter in [System.Net.NetworkInformation.NetworkInterface]::GetAllNetworkInterfaces()) {
    if (-not $adapter.Supports([System.Net.NetworkInformation.NetworkInterfaceComponent]::IPv4)) { continue }
    $properties = $adapter.GetIPProperties()
    if ($properties.GetIPv4Properties().Index -ne __IFINDEX__) { continue }
    if ($properties.DnsAddresses.Count -gt 0) {
        Set-DnsClientServerAddress -InterfaceIndex __IFINDEX__ -ResetServerAddresses
    }
    break
}
