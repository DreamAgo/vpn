import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
// 本地打包字体(离线可用):Archivo(界面) + IBM Plex Mono(数据)。
import '@fontsource/archivo/400.css';
import '@fontsource/archivo/500.css';
import '@fontsource/archivo/600.css';
import '@fontsource/ibm-plex-mono/400.css';
import '@fontsource/ibm-plex-mono/500.css';
import './styles/global.css';
import App from './App.tsx';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>
);
