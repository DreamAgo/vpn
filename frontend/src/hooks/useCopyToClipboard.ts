/**
 * 复制到剪贴板 hook（Story 3.8/3.9 复用）。
 *
 * - 优先使用 navigator.clipboard.writeText。
 * - 复制成功后短暂置 copied=true（约 2 秒自动恢复），供 UI 显示"✓ 已复制"。
 * - clipboard API 不可用（非 https / 旧浏览器）时 supported=false，由调用方展示 fallback。
 */
import { useCallback, useEffect, useRef, useState } from 'react';

export const clipboardSupported = (): boolean =>
  typeof navigator !== 'undefined' &&
  !!navigator.clipboard &&
  typeof navigator.clipboard.writeText === 'function';

export function useCopyToClipboard(resetDelay = 2000) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    []
  );

  const copy = useCallback(
    async (text: string): Promise<boolean> => {
      if (!clipboardSupported()) {
        return false;
      }
      try {
        await navigator.clipboard.writeText(text);
        setCopied(true);
        if (timer.current) clearTimeout(timer.current);
        timer.current = setTimeout(() => setCopied(false), resetDelay);
        return true;
      } catch {
        return false;
      }
    },
    [resetDelay]
  );

  return { copied, copy, supported: clipboardSupported() };
}
