/**
 * snake_case ↔ camelCase 转换（用于 axios 拦截器）。
 *
 * 后端 JSON 全部 snake_case，前端代码风格 camelCase，
 * 通过此模块在边界处统一转换。
 */

const snakeToCamel = (s: string): string => s.replace(/_([a-z])/g, (_, c) => c.toUpperCase());

const camelToSnake = (s: string): string => s.replace(/([A-Z])/g, '_$1').toLowerCase();

const isPlainObject = (v: unknown): v is Record<string, unknown> =>
  v !== null && typeof v === 'object' && !Array.isArray(v) && Object.getPrototypeOf(v) === Object.prototype;

/** 深度转换对象 key 从 snake_case 到 camelCase。 */
export function keysToCamel<T = unknown>(input: unknown): T {
  if (Array.isArray(input)) {
    return input.map((item) => keysToCamel(item)) as T;
  }
  if (isPlainObject(input)) {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(input)) {
      out[snakeToCamel(k)] = keysToCamel(v);
    }
    return out as T;
  }
  return input as T;
}

/** 深度转换对象 key 从 camelCase 到 snake_case。 */
export function keysToSnake<T = unknown>(input: unknown): T {
  if (Array.isArray(input)) {
    return input.map((item) => keysToSnake(item)) as T;
  }
  if (isPlainObject(input)) {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(input)) {
      out[camelToSnake(k)] = keysToSnake(v);
    }
    return out as T;
  }
  return input as T;
}
