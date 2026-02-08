#!/usr/bin/env node
// SPDX-License-Identifier: (MIT OR Apache-2.0)
// Build script: Generate examples.js from examples/*.rk files

const fs = require('fs').promises;
const path = require('path');

const EXAMPLES_DIR = path.join(__dirname, '../../examples');
const OUTPUT_FILE = path.join(__dirname, 'examples.js');

async function buildExamples() {
    try {
        // Read all .rk files
        const files = await fs.readdir(EXAMPLES_DIR);
        const rkFiles = files.filter(f => f.endsWith('.rk')).sort();

        console.log(`Found ${rkFiles.length} example files`);

        const examples = {};
        const metadata = [];

        for (const file of rkFiles) {
            const filePath = path.join(EXAMPLES_DIR, file);
            const content = await fs.readFile(filePath, 'utf-8');
            const key = path.basename(file, '.rk');

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
        console.log(`\n✅ Generated ${OUTPUT_FILE}`);
        console.log(`   ${Object.keys(examples).length} examples exported`);

    } catch (error) {
        console.error('❌ Error building examples:', error);
        process.exit(1);
    }
}

buildExamples();
