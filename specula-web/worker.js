// The stream worker: the same wasm as the page, with its own entry point.
// A page that names its glue in ?glue= and its wasm in ?wasm= gets the
// worker to load exactly those, so the two always match.
const params = new URL(location.href).searchParams;
const glue = params.get("glue") ?? "./pkg/specula_web.js";
const wasm = params.get("wasm");
const { default: init, worker_main } = await import(new URL(glue, location.href).href);
await init(wasm === null ? undefined : { module_or_path: new URL(wasm, location.href).href });
worker_main();
