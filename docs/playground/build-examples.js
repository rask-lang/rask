#!/usr/bin/env node
// SPDX-License-Identifier: (MIT OR Apache-2.0)
// Build script: Generate examples.js from examples/*.rk files

const fs = require('fs').promises;
const path = require('path');

const EXAMPLES_DIR = path.join(__dirname, '../../examples');
const OUTPUT_FILE = path.join(__dirname, 'examples.js');

// What the browser build can't do.
//
// The playground is the interpreter compiled to wasm32-unknown-unknown, which
// has no OS and no threads. Modules that need either are refused with a
// diagnostic (fs, io, net, http) or don't work at all (time, spawn), so an
// example that uses one can't run here and is left out of the dropdown. That's
// worked out from the source, not from a list someone has to keep updated.
const BROWSER_GAPS = [
    { pattern: /^\s*import\s+fs\b/m, reason: 'reads files' },
    { pattern: /^\s*import\s+io\b/m, reason: 'uses stdin/stdout directly' },
    { pattern: /^\s*import\s+net\b/m, reason: 'opens sockets' },
    { pattern: /^\s*import\s+http\b/m, reason: 'serves HTTP' },
    { pattern: /^\s*import\s+time\b|\btime\./m, reason: 'reads the clock' },
    { pattern: /\bspawn\s*\(|\bThreadPool\b|\bMultitasking\b/, reason: 'starts threads' },
];

function browserGap(source) {
    const reasons = BROWSER_GAPS.filter(g => g.pattern.test(source)).map(g => g.reason);
    if (reasons.length === 0) return null;
    return reasons.join(', ');
}

async function buildExamples() {
    try {
        // Read all .rk files
        const files = await fs.readdir(EXAMPLES_DIR);
        const rkFiles = files.filter(f => f.endsWith('.rk')).sort();

        console.log(`Found ${rkFiles.length} example files`);

        const examples = {};
        const metadata = [];
        let skipped = 0;

        for (const file of rkFiles) {
            const filePath = path.join(EXAMPLES_DIR, file);
            const content = await fs.readFile(filePath, 'utf-8');
            const key = path.basename(file, '.rk');

            const gap = browserGap(content);
            if (gap) {
                skipped++;
                console.log(`  - ${file}  skipped (${gap})`);
                continue;
            }

            // Extract title from filename (e.g., "hello_world" -> "Hello World")
            const title = key
                .split('_')
                .map(word => word.charAt(0).toUpperCase() + word.slice(1))
                .join(' ');

            examples[key] = content;
            metadata.push({ key, title, file });

            console.log(`  - ${file} -> ${key}`);
        }

        // Generate JavaScript file
        const output = `// SPDX-License-Identifier: (MIT OR Apache-2.0)
// Auto-generated from examples/*.rk files
// Run: node build-examples.js

export const EXAMPLES = ${JSON.stringify(examples, null, 4)};

export const EXAMPLE_METADATA = ${JSON.stringify(metadata, null, 4)};

export const DEFAULT_CODE = EXAMPLES.hello_world || \`func main() {
    println("Hello, World!")
}\`;
`;

        await fs.writeFile(OUTPUT_FILE, output, 'utf-8');
        console.log(`\nGenerated ${OUTPUT_FILE}`);
        console.log(`   ${Object.keys(examples).length} examples, ${skipped} skipped as browser-impossible`);

    } catch (error) {
        console.error('Error building examples:', error);
        process.exit(1);
    }
}

buildExamples();
