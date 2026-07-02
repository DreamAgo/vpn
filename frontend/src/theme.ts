import { theme as antdTheme, type ThemeConfig } from 'antd';

const fontFamily = '"Archivo", system-ui, -apple-system, BlinkMacSystemFont, "PingFang SC", sans-serif';
const codeFontFamily = '"IBM Plex Mono", ui-monospace, "SF Mono", Menlo, monospace';

export type ThemeMode = 'light' | 'dark';

export interface ThemePalette {
  label: string;
  primary: string;
  primaryHover: string;
  primaryActive: string;
  wash: string;
  glow: string;
  rgb: string;
}

export interface ThemeSurfaces {
  bg: string;
  card: string;
  cardAlt: string;
  elevated: string;
  ink: string;
  inkSoft: string;
  inkFaint: string;
  line: string;
  lineStrong: string;
  tableHeader: string;
  tableHover: string;
  menuHover: string;
  codeBg: string;
  codeBorder: string;
  codeText: string;
  shadow: string;
  shadowSecondary: string;
}

export const defaultThemePalette: ThemePalette = {
  label: '蓝色',
  primary: '#2563eb',
  primaryHover: '#1d4ed8',
  primaryActive: '#1e40af',
  wash: '#eff6ff',
  glow: 'rgba(37, 99, 235, 0.14)',
  rgb: '37, 99, 235',
};

export const DEFAULT_THEME_MODE: ThemeMode = 'light';

export function isThemeMode(value: string | null): value is ThemeMode {
  return value === 'light' || value === 'dark';
}

export function getThemeSurfaces(mode: ThemeMode): ThemeSurfaces {
  if (mode === 'dark') {
    return {
      bg: '#0f172a',
      card: '#111827',
      cardAlt: '#172033',
      elevated: '#1f2937',
      ink: '#f8fafc',
      inkSoft: '#cbd5e1',
      inkFaint: '#64748b',
      line: '#263244',
      lineStrong: '#334155',
      tableHeader: '#172033',
      tableHover: '#18243a',
      menuHover: '#18243a',
      codeBg: '#0b1120',
      codeBorder: '#1e293b',
      codeText: '#e2e8f0',
      shadow: '0 1px 2px rgba(0,0,0,0.32)',
      shadowSecondary: '0 18px 45px rgba(0,0,0,0.35)',
    };
  }

  return {
    bg: '#f5f7fb',
    card: '#ffffff',
    cardAlt: '#f8fafc',
    elevated: '#ffffff',
    ink: '#111827',
    inkSoft: '#4b5563',
    inkFaint: '#9ca3af',
    line: '#e8edf5',
    lineStrong: '#d8dee8',
    tableHeader: '#f8fafc',
    tableHover: '#f8fbff',
    menuHover: '#f3f6fb',
    codeBg: '#f1f5f9',
    codeBorder: '#e2e8f0',
    codeText: '#334155',
    shadow: '0 1px 2px rgba(16,24,40,0.06)',
    shadowSecondary: '0 18px 45px rgba(15,23,42,0.08)',
  };
}

function getAccentWash(palette: ThemePalette, mode: ThemeMode) {
  return mode === 'dark' ? `rgba(${palette.rgb}, 0.16)` : palette.wash;
}

function getAccentGlow(palette: ThemePalette, mode: ThemeMode) {
  return mode === 'dark' ? `rgba(${palette.rgb}, 0.22)` : palette.glow;
}

export function applyThemeAppearance(palette: ThemePalette, mode: ThemeMode) {
  const root = document.documentElement;
  const surfaces = getThemeSurfaces(mode);

  root.dataset.themeMode = mode;
  root.style.setProperty('--bg-0', surfaces.elevated);
  root.style.setProperty('--bg-1', surfaces.bg);
  root.style.setProperty('--card', surfaces.card);
  root.style.setProperty('--card-2', surfaces.cardAlt);
  root.style.setProperty('--ink', surfaces.ink);
  root.style.setProperty('--ink-soft', surfaces.inkSoft);
  root.style.setProperty('--ink-faint', surfaces.inkFaint);
  root.style.setProperty('--line', surfaces.line);
  root.style.setProperty('--line-strong', surfaces.lineStrong);
  root.style.setProperty('--code-bg', surfaces.codeBg);
  root.style.setProperty('--code-border', surfaces.codeBorder);
  root.style.setProperty('--code-text', surfaces.codeText);
  root.style.setProperty('--surface-shadow', surfaces.shadow);
  root.style.setProperty('--surface-shadow-secondary', surfaces.shadowSecondary);

  root.style.setProperty('--accent', palette.primary);
  root.style.setProperty('--accent-2', palette.primaryHover);
  root.style.setProperty('--accent-deep', palette.primaryActive);
  root.style.setProperty('--accent-glow', getAccentGlow(palette, mode));
  root.style.setProperty('--accent-wash', getAccentWash(palette, mode));
  root.style.setProperty('--accent-rgb', palette.rgb);
}

export function createAppTheme(palette: ThemePalette, mode: ThemeMode): ThemeConfig {
  const surfaces = getThemeSurfaces(mode);
  const accentWash = getAccentWash(palette, mode);

  return {
    algorithm: mode === 'dark' ? antdTheme.darkAlgorithm : antdTheme.defaultAlgorithm,
    token: {
      colorPrimary: palette.primary,
      colorInfo: palette.primary,
      colorSuccess: '#16a34a',
      colorWarning: '#d97706',
      colorError: '#dc2626',
      colorLink: palette.primary,

      colorBgLayout: surfaces.bg,
      colorBgContainer: surfaces.card,
      colorBgElevated: surfaces.elevated,
      colorBgSpotlight: accentWash,

      colorText: surfaces.ink,
      colorTextSecondary: surfaces.inkSoft,
      colorTextTertiary: surfaces.inkFaint,
      colorBorder: surfaces.lineStrong,
      colorBorderSecondary: surfaces.line,

      borderRadius: 6,
      fontFamily,
      fontSize: 14,
      wireframe: false,
      boxShadow: surfaces.shadow,
      boxShadowSecondary: surfaces.shadowSecondary,
    },
    components: {
      Layout: {
        bodyBg: surfaces.bg,
        headerBg: surfaces.card,
        siderBg: surfaces.card,
        headerHeight: 60,
      },
      Menu: {
        itemBg: 'transparent',
        subMenuItemBg: 'transparent',
        itemColor: surfaces.inkSoft,
        itemSelectedColor: mode === 'dark' ? palette.primary : palette.primaryHover,
        itemSelectedBg: accentWash,
        itemHoverColor: surfaces.ink,
        itemHoverBg: surfaces.menuHover,
        itemBorderRadius: 6,
        itemHeight: 40,
        fontSize: 13,
      },
      Table: {
        headerBg: surfaces.tableHeader,
        headerColor: surfaces.inkSoft,
        rowHoverBg: surfaces.tableHover,
        borderColor: surfaces.line,
        cellPaddingBlock: 12,
      },
      Card: { colorBorderSecondary: surfaces.line, paddingLG: 20 },
      Button: { primaryShadow: 'none', fontWeight: 600, controlHeight: 36, borderRadius: 6 },
      Statistic: { contentFontSize: 30 },
      Input: { controlHeight: 36, activeShadow: `0 0 0 3px rgba(${palette.rgb},0.14)` },
      Select: { controlHeight: 36 },
      Tag: {
        defaultBg: mode === 'dark' ? '#1f2937' : '#f3f4f6',
        defaultColor: surfaces.inkSoft,
      },
      Descriptions: { labelBg: surfaces.tableHeader },
      Modal: { contentBg: surfaces.card, headerBg: surfaces.card },
    },
  };
}

export const theme = createAppTheme(defaultThemePalette, DEFAULT_THEME_MODE);

export { fontFamily, codeFontFamily };
