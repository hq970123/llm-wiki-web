/**
 * Web shim for `@tauri-apps/plugin-store`. Stores live on the server
 * (DATA_DIR/app/<name>), the same file the Rust backend reads.
 */
import { apiJson } from "./runtime"

export interface StoreOptions {
  defaults?: Record<string, unknown>
  autoSave?: boolean | number
  createNew?: boolean
  overrideDefaults?: boolean
}

async function op<T>(name: string, o: string, extra: Record<string, unknown> = {}): Promise<T> {
  const res = await apiJson<{ value: T }>("/api/store", { name, op: o, ...extra })
  return res.value
}

export class Store {
  constructor(public readonly path: string) {}

  static async load(path: string, options?: StoreOptions): Promise<Store> {
    await op(path, "load", { defaults: options?.defaults ?? null })
    return new Store(path)
  }

  static async get(path: string): Promise<Store | null> {
    return new Store(path)
  }

  async get<T>(key: string): Promise<T | undefined> {
    const v = await op<T | null>(this.path, "get", { key })
    return v === null ? undefined : v
  }
  async set(key: string, value: unknown): Promise<void> {
    await op(this.path, "set", { key, value: value === undefined ? null : value })
  }
  async has(key: string): Promise<boolean> {
    return op<boolean>(this.path, "has", { key })
  }
  async delete(key: string): Promise<boolean> {
    return op<boolean>(this.path, "delete", { key })
  }
  async clear(): Promise<void> {
    await op(this.path, "clear")
  }
  async reset(): Promise<void> {
    await op(this.path, "reset")
  }
  async keys(): Promise<string[]> {
    return op<string[]>(this.path, "keys")
  }
  async values<T>(): Promise<T[]> {
    return op<T[]>(this.path, "values")
  }
  async entries<T>(): Promise<Array<[key: string, value: T]>> {
    return op<Array<[string, T]>>(this.path, "entries")
  }
  async length(): Promise<number> {
    return op<number>(this.path, "length")
  }
  async reload(): Promise<void> {
    await op(this.path, "reload")
  }
  async save(): Promise<void> {
    await op(this.path, "save")
  }
  async onKeyChange<T>(_key: string, _cb: (value: T | undefined) => void): Promise<() => void> {
    return () => {}
  }
  async onChange<T>(_cb: (key: string, value: T | undefined) => void): Promise<() => void> {
    return () => {}
  }
  async close(): Promise<void> {}
}

export class LazyStore extends Store {
  constructor(path: string, private options?: StoreOptions) {
    super(path)
  }
  async init(): Promise<void> {
    await Store.load(this.path, this.options)
  }
}

export async function load(path: string, options?: StoreOptions): Promise<Store> {
  return Store.load(path, options)
}

export async function getStore(path: string): Promise<Store | null> {
  return Store.get(path)
}
