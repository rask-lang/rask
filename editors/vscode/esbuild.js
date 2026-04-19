// Bundles extension.ts + vscode-languageclient + transitive deps into a
// single `out/extension.js` so the .vsix doesn't ship node_modules.
// `vscode` itself is external — it's provided by the host at runtime.
const esbuild = require('esbuild');

const production = process.argv.includes('--production');
const watch = process.argv.includes('--watch');

async function main() {
  const ctx = await esbuild.context({
    entryPoints: ['src/extension.ts'],
    bundle: true,
    format: 'cjs',
    minify: production,
    sourcemap: !production,
    sourcesContent: false,
    platform: 'node',
    target: 'node20',
    outfile: 'out/extension.js',
    external: ['vscode'],
    logLevel: 'warning',
  });
  if (watch) {
    await ctx.watch();
  } else {
    await ctx.rebuild();
    await ctx.dispose();
  }
}

main().catch(e => { console.error(e); process.exit(1); });
