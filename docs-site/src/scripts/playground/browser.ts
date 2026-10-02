import compilerUrl from '../../generated/assemblyscript/asc.js?url';
import { browserCompilerWorker, createCompiler } from './compiler';
import { FUNCTIONS } from './functions';
import { loadKernel, loadPlaygroundVerifier } from './kernel';
import { browserWorker, createWorkerRunner } from './runner';
import { initPlayground } from './ui';

/** Hands the person a file, as a link they did not have to click. */
function save(name: string, bytes: Uint8Array) {
  const url = URL.createObjectURL(new Blob([bytes as BlobPart], { type: 'application/wasm' }));
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/**
 * The playground as the page runs it: the WebAssembly kernel and verifier, the AssemblyScript compiler (a file of the
 * site's own, fetched only when someone compiles), and real Workers.
 */
export function startPlayground(scope: ParentNode = document): void {
  initPlayground(scope, {
    loadKernel,
    loadVerifier: loadPlaygroundVerifier,
    runModule: createWorkerRunner(browserWorker),
    functions: FUNCTIONS,
    compiler: createCompiler({ spawn: browserCompilerWorker, compilerUrl: new URL(compilerUrl, location.href).href }),
    download: save,
  });
}
