export type StartSessionCommand = {
  id: string;
  cmd: "start_session";
  session_id?: string;
  profile?: string;
  url: string;
  headed?: boolean;
  user_data_dir?: string;
  browser_channel?: "chromium" | "chrome" | "msedge";
  wait_for?: string;
  timeout_ms?: number;
};

export type ConnectOverCdpCommand = {
  id: string;
  cmd: "connect_over_cdp";
  session_id?: string;
  profile?: string;
  cdp_url: string;
  page_url_contains?: string;
  wait_for?: string;
  timeout_ms?: number;
};

export type OpenPageCommand = {
  id: string;
  cmd: "open_page";
  session_id: string;
  url: string;
  timeout_ms?: number;
};

export type ProbePageCommand = {
  id: string;
  cmd: "probe_page";
  session_id: string;
  profile?: string;
  timeout_ms?: number;
};

export type ClosePageCommand = {
  id: string;
  cmd: "close_page";
  session_id: string;
};

export type SendChatCommand = {
  id: string;
  cmd: "send_chat";
  session_id: string;
  text: string;
  input_selector?: string;
  submit_selector?: string;
  timeout_ms?: number;
};

export type UploadFileCommand = {
  id: string;
  cmd: "upload_file";
  session_id: string;
  file_path: string;
  input_selector?: string;
  button_selector?: string;
  timeout_ms?: number;
};

export type ReadResponseCommand = {
  id: string;
  cmd: "read_response";
  session_id: string;
  response_selector?: string;
  timeout_ms?: number;
  idle_ms?: number;
};

export type CloseSessionCommand = {
  id: string;
  cmd: "close_session";
  session_id: string;
};

export type BridgeCommand =
  | StartSessionCommand
  | ConnectOverCdpCommand
  | OpenPageCommand
  | ProbePageCommand
  | ClosePageCommand
  | SendChatCommand
  | UploadFileCommand
  | ReadResponseCommand
  | CloseSessionCommand;

export type OkResponse = {
  id: string;
  ok: true;
  cmd: BridgeCommand["cmd"];
  session_id?: string;
  data?: unknown;
};

export type ErrResponse = {
  id: string;
  ok: false;
  cmd?: BridgeCommand["cmd"];
  session_id?: string;
  error: string;
};

export type BridgeResponse = OkResponse | ErrResponse;
