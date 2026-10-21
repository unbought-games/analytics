import {build} from 'esbuild';
const common = {entryPoints: ['src/index.ts'], bundle: true, minify: true, target: ['es2022'], legalComments: 'none'};
await Promise.all([
  build({...common, format: 'esm', outfile: 'dist/index.js'}),
  build({...common, format: 'cjs', outfile: 'dist/index.cjs'}),
  build({...common, format: 'iife', globalName: 'Analytics', outfile: 'dist/analytics.global.js'})
]);
