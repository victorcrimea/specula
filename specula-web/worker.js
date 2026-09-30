// The stream worker: the same wasm as the page, with its own entry point.
import init, { worker_main } from "./pkg/specula_web.js";
await init();
worker_main();
