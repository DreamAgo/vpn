/**
 * AntD 5 主题 token 配置。
 *
 * 视觉策略（来自 UX Spec §Visual Design Foundation）：
 * - 主色 geekblue #2F54EB（技术专业感，避免营销 SaaS 浅蓝）
 * - 4 状态色：成功绿/警告黄/错误红/信息蓝
 * - 深色侧边栏 + 浅色主区
 * - 较小圆角（4px，工具感）
 * - 系统字体优先（无 Web Font）
 */
import type { ThemeConfig } from 'antd';

const fontFamily =
  '-apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", "Helvetica Neue", Helvetica, Arial, sans-serif';

const codeFontFamily =
  '"SF Mono", "Monaco", "Cascadia Code", "Roboto Mono", Consolas, "Liberation Mono", "Source Code Pro", "Menlo", "Courier New", monospace';

export const theme: ThemeConfig = {
  token: {
    colorPrimary: '#2F54EB',
    colorSuccess: '#52C41A',
    colorWarning: '#FAAD14',
    colorError: '#FF4D4F',
    colorInfo: '#1677FF',
    borderRadius: 4,
    fontFamily,
  },
  components: {
    Layout: {
      siderBg: '#001529',
      headerBg: '#FFFFFF',
    },
    Table: {
      rowHoverBg: '#FAFAFA',
      headerBg: '#FAFAFA',
    },
  },
};

export { fontFamily, codeFontFamily };
