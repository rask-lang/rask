#!/usr/bin/env node
// SPDX-License-Identifier: (MIT OR Apache-2.0)
// Build script: Generate examples.js from examples/*.rk files

import fs from 'fs/promises';
import path from 'path';
import { fileURLToPath } from 'url';

const { readdir, readFile, writeFile } = fs;
const { join, basename, dirname } = path;

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const EXAMPLES_DIR = join(__dirname, '../../examples');
const OUTPUT_FILE = join(__dirname, 'examples.js');

async function buildExamples() {
    try {
        // Read all .rk files
        const files = await readdir(EXAMPLES_DIR);
        const rkFiles = files.filter(f => f.endsWith('.rk')).sort();

        console.log(`Found ${rkFiles.length} example files`);

        const examples = {};
        const metadata = [];

        for (const file of rkFiles) {
            const path = join(EXAMPLES_DIR, file);
            const content = await readFile(path, 'utf-8');
            const key = basename(file, '.rk');

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
// Run: node build-examples.mjs

export const EXAMPLES = ${JSON.stringify(examples, null, 4)};

export const EXAMPLE_METADATA = ${JSON.stringify(metadata, null, 4)};

export const DEFAULT_CODE = EXAMPLES.hello_world || \`func main() {
    println("Hello, World!")
}\`;
`;

        await writeFile(OUTPUT_FILE, output, 'utf-8');
        console.log(`\n✅ Generated ${OUTPUT_FILE}`);
        console.log(`   ${Object.keys(examples).length} examples exported`);

    } catch (error) {
        console.error('❌ Error building examples:', error);
        process.exit(1);
    }
}

buildExamples();
