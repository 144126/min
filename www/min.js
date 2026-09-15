const SIMD = new Uint8Array([
  0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 123, 3, 2, 1, 0, 10, 10, 1, 8,
  0, 65, 0, 253, 15, 253, 98, 11,
]);

function has_simd() {
  try {
    return WebAssembly.validate(SIMD);
  } catch {
    return false;
  }
}

function has_jspi() {
  return typeof WebAssembly.Suspending === "function" && typeof WebAssembly.promising === "function";
}

let mem;
let exp;

function view(p, n) {
  return new Uint8Array(mem.buffer, p, n);
}

function write_bytes(u8) {
  const p = exp.min_alloc(u8.length);
  view(p, u8.length).set(u8);
  return p;
}

function read_pack(pack) {
  const p = Number(pack >> 32n);
  const n = Number(pack & 0xffffffffn);
  const out = new Uint8Array(n);
  out.set(view(p, n));
  exp.min_free(p, n);
  return out;
}

async function do_fetch(p, n) {
  const req = JSON.parse(new TextDecoder().decode(view(p, n)));
  const init = { method: req.method || "GET", headers: req.headers || {} };
  if (req.body !== undefined && req.body !== null) {
    init.body = typeof req.body === "string" ? req.body : JSON.stringify(req.body);
  }
  const r = await fetch(req.url, init);
  const buf = new Uint8Array(await r.arrayBuffer());
  const op = write_bytes(buf);
  return (BigInt(op) << 32n) | BigInt(buf.length);
}

async function boot() {
  const url = has_simd() ? "min-simd.wasm" : "min.wasm";
  const imports = { env: {} };
  if (has_jspi()) {
    imports.env.min_fetch = new WebAssembly.Suspending(do_fetch);
  } else {
    imports.env.min_fetch = () => {
      throw new Error("jspi missing; use run_js");
    };
  }
  const { instance } = await WebAssembly.instantiateStreaming(fetch(url), imports);
  exp = instance.exports;
  mem = exp.memory;
  return { jspi: has_jspi(), simd: has_simd(), url };
}

async function run_jspi(job) {
  const u8 = new TextEncoder().encode(JSON.stringify(job));
  const p = write_bytes(u8);
  const fn = WebAssembly.promising(exp.min_run);
  const pack = await fn(p, u8.length);
  exp.min_free(p, u8.length);
  return new TextDecoder().decode(read_pack(BigInt(pack)));
}

self.onmessage = async (e) => {
  const m = e.data || {};
  try {
    if (m.t === "boot") {
      self.postMessage({ t: "ready", ...(await boot()) });
      return;
    }
    if (m.t === "run") {
      if (!has_jspi()) {
        self.postMessage({ t: "err", e: "WebAssembly.Suspending missing; JS-owned loop not wired" });
        return;
      }
      const out = await run_jspi(m.job);
      self.postMessage({ t: "out", out });
      return;
    }
  } catch (err) {
    self.postMessage({ t: "err", e: String(err && err.message ? err.message : err) });
  }
};
