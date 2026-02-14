import {
    workspace, window, commands, languages,
    ExtensionContext, TextDocument, TextEdit, Range,
} from 'vscode';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
} from 'vscode-languageclient/node';
import { execFile } from 'child_process';

let client: LanguageClient | undefined;

function getRaskPath(): string {
    const config = workspace.getConfiguration('rask');
    const serverPath = config.get<string>('serverPath');
    if (serverPath) {
        // serverPath points to rask-lsp; assume rask is next to it
        const dir = serverPath.substring(0, serverPath.lastIndexOf('/'));
        if (dir) {
            return dir + '/rask';
        }
    }
    return 'rask';
}

function startClient(): void {
    const config = workspace.getConfiguration('rask');
    let serverPath = config.get<string>('serverPath');

    if (!serverPath) {
        serverPath = 'rask-lsp';
    }

    const serverOptions: ServerOptions = {
        command: serverPath,
        args: [],
    };

    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: 'file', language: 'rask' }],
        synchronize: {
            fileEvents: workspace.createFileSystemWatcher('**/*.rk'),
        },
    };

    client = new LanguageClient(
        'rask',
        'Rask Language Server',
        serverOptions,
        clientOptions
    );

    client.start().catch(() => {
        window.showWarningMessage(
            'Could not start rask-lsp. Install it or set rask.serverPath in settings. ' +
            'Syntax highlighting still works without it.'
        );
    });
}

export function activate(context: ExtensionContext) {
    startClient();

    context.subscriptions.push(
        commands.registerCommand('rask.run', () => {
            const editor = window.activeTextEditor;
            if (!editor || editor.document.languageId !== 'rask') {
                window.showWarningMessage('Open a .rk file to run it.');
                return;
            }
            const filePath = editor.document.fileName;
            const terminal = window.createTerminal('Rask');
            terminal.show();
            terminal.sendText(`${getRaskPath()} run "${filePath}"`);
        }),

        commands.registerCommand('rask.restartServer', async () => {
            if (client) {
                await client.stop();
                client = undefined;
            }
            startClient();
        }),

        languages.registerDocumentFormattingEditProvider('rask', {
            provideDocumentFormattingEdits(document: TextDocument): Promise<TextEdit[]> {
                return new Promise((resolve, reject) => {
                    execFile(getRaskPath(), ['fmt', document.fileName], (err, stdout) => {
                        if (err) {
                            window.showWarningMessage(`rask fmt failed: ${err.message}`);
                            resolve([]);
                            return;
                        }
                        const fullRange = new Range(
                            document.positionAt(0),
                            document.positionAt(document.getText().length)
                        );
                        resolve([TextEdit.replace(fullRange, stdout)]);
                    });
                });
            }
        })
    );
}

export function deactivate(): Thenable<void> | undefined {
    if (!client) {
        return undefined;
    }
    return client.stop();
}
