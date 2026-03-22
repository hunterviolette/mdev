export type ConnectCommand = {
  id: string;
  cmd: "connect";
  session_id?: string;
  base_url: string;
  auth_type?: "basic" | "header" | "negotiate" | "cookie";
  transport?: "fetch" | "curl";
  username?: string;
  password?: string;
  authorization?: string;
  cookie_header?: string;
  negotiate_command?: string;
  client?: string;
  timeout_ms?: number;
};

export type ListPackageObjectsCommand = {
  id: string;
  cmd: "list_package_objects";
  session_id: string;
  package_name: string;
  include_subpackages?: boolean;
};

export type ReadObjectCommand = {
  id: string;
  cmd: "read_object";
  session_id: string;
  object_uri: string;
  accept?: string;
};

export type LockObjectCommand = {
  id: string;
  cmd: "lock_object";
  session_id: string;
  object_uri: string;
};

export type UnlockObjectCommand = {
  id: string;
  cmd: "unlock_object";
  session_id: string;
  object_uri: string;
  lock_handle: string;
};

export type UpdateObjectCommand = {
  id: string;
  cmd: "update_object";
  session_id: string;
  object_uri: string;
  source: string;
  content_type?: string;
  lock_handle?: string;
  corr_nr?: string;
  headers?: Record<string, string>;
};

export type CreateObjectCommand = {
  id: string;
  cmd: "create_object";
  session_id: string;
  collection_uri: string;
  body: string;
  content_type?: string;
  accept?: string;
  headers?: Record<string, string>;
};

export type CreateTransportCommand = {
  id: string;
  cmd: "create_transport";
  session_id: string;
  collection_uri: string;
  body: string;
  content_type?: string;
  accept?: string;
  headers?: Record<string, string>;
};

export type CallEndpointCommand = {
  id: string;
  cmd: "call_endpoint";
  session_id: string;
  method: "GET" | "POST" | "PUT" | "PATCH" | "DELETE";
  uri: string;
  body?: string;
  content_type?: string;
  accept?: string;
  headers?: Record<string, string>;
};

export type SyntaxCheckCommand = {
  id: string;
  cmd: "syntax_check";
  session_id: string;
  object_uri: string;
};

export type ActivateObjectCommand = {
  id: string;
  cmd: "activate_object";
  session_id: string;
  object_uri: string;
};

export type ActivatePackageCommand = {
  id: string;
  cmd: "activate_package";
  session_id: string;
  package_name: string;
};

export type GetProblemsCommand = {
  id: string;
  cmd: "get_problems";
  session_id: string;
  result_uri?: string;
  xml?: string;
};

export type CloseSessionCommand = {
  id: string;
  cmd: "close_session";
  session_id: string;
};

export type AdtCommand =
  | ConnectCommand
  | ListPackageObjectsCommand
  | ReadObjectCommand
  | LockObjectCommand
  | UnlockObjectCommand
  | UpdateObjectCommand
  | CreateObjectCommand
  | CreateTransportCommand
  | CallEndpointCommand
  | SyntaxCheckCommand
  | ActivateObjectCommand
  | ActivatePackageCommand
  | GetProblemsCommand
  | CloseSessionCommand;

export type OkResponse = {
  id: string;
  ok: true;
  cmd: AdtCommand["cmd"];
  session_id?: string;
  data?: unknown;
};

export type ErrResponse = {
  id: string;
  ok: false;
  cmd?: AdtCommand["cmd"];
  session_id?: string;
  error: string;
};

export type AdtResponse = OkResponse | ErrResponse;