/* tslint:disable */
/* eslint-disable */
export class Client {
  free(): void;
  constructor();
  set_url(url: string): void;
  set_info(info: string): void;
  add_peer_info(peer_id: string, info: string): void;
  getAllInfo(): Map<any, any>;
  add_super_peer(peer: string, info: string): void;
  add_child_peer(peer: string, info: string): void;
  remove_peer(peer: string): void;
  get_peers_to_retry(): (string)[];
  start(): void;
  readonly info: string;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly memory: WebAssembly.Memory;
  readonly __wbg_client_free: (a: number, b: number) => void;
  readonly client_new: () => number;
  readonly client_set_url: (a: number, b: number, c: number) => void;
  readonly client_set_info: (a: number, b: number, c: number) => void;
  readonly client_info: (a: number) => [number, number];
  readonly client_add_peer_info: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly client_getAllInfo: (a: number) => any;
  readonly client_add_super_peer: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly client_add_child_peer: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly client_remove_peer: (a: number, b: number, c: number) => void;
  readonly client_get_peers_to_retry: (a: number) => [number, number];
  readonly client_start: (a: number) => [number, number];
  readonly __wbindgen_exn_store: (a: number) => void;
  readonly __externref_table_alloc: () => number;
  readonly __wbindgen_export_2: WebAssembly.Table;
  readonly __wbindgen_free: (a: number, b: number, c: number) => void;
  readonly __wbindgen_malloc: (a: number, b: number) => number;
  readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
  readonly __wbindgen_export_6: WebAssembly.Table;
  readonly __externref_drop_slice: (a: number, b: number) => void;
  readonly __externref_table_dealloc: (a: number) => void;
  readonly closure219_externref_shim: (a: number, b: number, c: any) => void;
  readonly closure246_externref_shim: (a: number, b: number, c: any) => void;
  readonly closure259_externref_shim: (a: number, b: number, c: any) => void;
  readonly _dyn_core__ops__function__FnMut_____Output___R_as_wasm_bindgen__closure__WasmClosure___describe__invoke__h6fa27522f81883d4: (a: number, b: number) => void;
  readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;
/**
* Instantiates the given `module`, which can either be bytes or
* a precompiled `WebAssembly.Module`.
*
* @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
*
* @returns {InitOutput}
*/
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
* If `module_or_path` is {RequestInfo} or {URL}, makes a request and
* for everything else, calls `WebAssembly.instantiate` directly.
*
* @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
*
* @returns {Promise<InitOutput>}
*/
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
