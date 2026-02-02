import * as path from 'path';
import { workspace, ExtensionContext } from 'vscode';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
} from 'vscode-languageclient/node';

let client: LanguageClient | undefined;

export function activate(context: ExtensionContext) {
    const config = workspace.getConfiguration('rask');
    let serverPath = config.get<string>('serverPath');

    if (!serverPath) {
        // Default: look for rask-lsp in PATH or relative to extension
        serverPath = 'rask-lsp';
    }

    const serverOptions: ServerOptions = {
        command: serverPath,
        args: [],
    };

    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: 'file', language: 'rask' }],
        synchronize: {
            fileEvents: workspace.createFileSystemWatcher('**/*.rask'),
        },
    };

    client = new LanguageClient(
        'rask',
        'Rask Language Server',
        serverOptions,
        clientOptions
    );

    client.start();
}

export function deactivate(): Thenable<void> | undefined {
    if (!client) {
        return undefined;
    }
    return client.stop();
}
