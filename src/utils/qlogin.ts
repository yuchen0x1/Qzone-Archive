import { invoke } from "@tauri-apps/api/core";

// 登录凭证（Cookie/g_tk/User-Agent）只存在于 Rust 后端内存中，
// 前端仅持有不含敏感信息的账号标识。
export interface LoginCredentials {
  uin: string;
}

export interface WebLoginStatus {
  status: "success" | "error" | "webLoginOpened" | "webLoginWaiting" | "webLoginCancelled";
  message: string;
  auth?: LoginCredentials;
}

export const openWebLogin = () => invoke<WebLoginStatus>("open_web_login");
export const checkWebLogin = () => invoke<WebLoginStatus>("check_web_login");
export const syncCookiesToWebview = () => invoke<void>("sync_cookies_to_webview");

export interface QzoneLoginUser {
  uin: string;
  nickname: string;
  avatarImage?: string;
}

// 用户资料与头像由后端携带会话凭证代为获取，前端不再接触 Cookie。
export async function getQzoneLoginUser(): Promise<QzoneLoginUser> {
  return invoke<QzoneLoginUser>("get_login_user");
}
