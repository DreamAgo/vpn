/**
 * IPv4 CIDR 校验（前端即时反馈用）。组路由 / 服务端 LAN / 节点路由 / 网段目录共用，
 * 避免各页各自复制一份正则导致规则漂移。后端 normalize_subnets 为最终权威校验。
 */

/**
 * 校验单个 IPv4 CIDR：a.b.c.d/n。规则与后端 `normalize_subnets`（Ipv4Net::parse）保持一致，
 * 避免 UI 放行后端会拒的值、提交后才报 400：
 * - 每段 0-255，且**无前导零**（"192.168.001.0" 后端 Ipv4Addr 解析会拒）；
 * - 前缀 **1-32**（后端拒绝 0.0.0.0/0 全隧道，故前缀 0 也拒）。
 */
export function isValidCidr(cidr: string): boolean {
  const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\/(\d{1,2})$/.exec(cidr.trim());
  if (!m) return false;
  const octets = [m[1], m[2], m[3], m[4]];
  // 0-255 且无前导零：String(Number("01")) === "1" !== "01" → 拒；"0" 自身合法。
  if (octets.some((o) => Number(o) > 255 || String(Number(o)) !== o)) return false;
  const prefix = Number(m[5]);
  return prefix >= 1 && prefix <= 32;
}
