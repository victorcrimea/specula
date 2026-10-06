// The stream worker: the same wasm as the page, with its own entry point.
// A page that names its glue in ?glue= gets the worker to import exactly
// that one, so the two always match.
const glue = new URL(location.href).searchParams.get("glue") ?? "./pkg/specula_web.js";
const { default: init, worker_main } = await import(new URL(glue, location.href).href);
await init();
worker_main();
