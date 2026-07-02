-- 用户多终端模式：每用户可注册的终端（peer）数量上限，默认 1。
-- 达上限后新终端注册会接管最旧的不在线终端；全部在线则拒绝注册。
ALTER TABLE users ADD COLUMN max_devices INTEGER NOT NULL DEFAULT 1 CHECK (max_devices >= 1);
