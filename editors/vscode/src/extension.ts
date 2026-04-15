import {
    workspace, window, commands, languages,
    ExtensionContext, TextDocument, TextEdit, Range,
    StatusBarAlignment, StatusBarItem, ThemeColor, Uri,
} from 'vscode';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
    RevealOutputChannelOn,
    State,
} from 'vscode-languageclient/node';
import { execFile, spawn } from 'child_process';
import * as path from 'path';
import * as fs from 'fs';

let client: LanguageClient | undefined;
let statusBar: StatusBarItem | undefined;

function getConfig() {
    return workspace.getConfiguration('rask');
}

function resolveServerPath(): string {
    const configured = getConfig().get<string>('serverPath');
    if (configured && configured.trim().length > 0) {
        return configured;
    }
    return 'rask-lsp';
}

function resolveRaskPath(): string {
    const configured = getConfig().get<string>('cliPath');
    if (configured && configured.trim().length > 0) {
        return configured;
    }
    // If serverPath is set, assume `rask` is a sibling.
    const serverPath = getConfig().get<string>('serverPath');
    if (serverPath && serverPath.trim().length > 0) {
        const dir = path.dirname(serverPath);
        const candidate = path.join(dir, 'rask');
        if (fs.existsSync(candidate)) {
            return candidate;
        }
    }
    return 'rask';
}

function setStatus(kind: 'starting' | 'ready' | 'error' | 'stopped', detail?: string) {
    if (!statusBar) return;
    switch (kind) {
        case 'starting':
            statusBar.text = '$(sync~spin) Rask: starting';
            statusBar.backgroundColor = undefined;
            break;
        case 'ready':
            statusBar.text = '$(check) Rask';
            statusBar.backgroundColor = undefined;
            break;
        case 'error':
            statusBar.text = '$(error) Rask';
            statusBar.backgroundColor = new ThemeColor('statusBarItem.errorBackground');
            break;
        case 'stopped':
            statusBar.text = '$(circle-slash) Rask';
            statusBar.backgroundColor = new ThemeColor('statusBarItem.warningBackground');
            break;
    }
    statusBar.tooltip = detail ? `Rask LSP — ${kind}\n${detail}` : `Rask LSP — ${kind}\nClick to restart`;
    statusBar.show();
}

async function startClient(context: ExtensionContext): Promise<void> {
    const serverPath = resolveServerPath();

    const serverOptions: ServerOptions = {
        command: serverPath,
        args: [],
        options: {
            env: {
                ...process.env,
                RUST_BACKTRACE: '1',
            },
        },
    };

    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: 'file', language: 'rask' }],
        synchronize: {
            fileEvents: workspace.createFileSystemWatcher('**/*.rk'),
        },
        // Send startup failures to the "Rask Language Server" output channel
        // instead of a modal popup — the user can inspect the actual error.
        revealOutputChannelOn: RevealOutputChannelOn.Error,
        initializationOptions: {
            // Advertise UTF-16 so positions align with the editor.
            positionEncodings: ['utf-16'],
        },
        markdown: {
            isTrusted: false,
            supportHtml: false,
        },
    };

    client = new LanguageClient(
        'rask',
        'Rask Language Server',
        serverOptions,
        clientOptions,
    );

    // Wire state transitions to the status bar so the user can see when
    // the analyzer is alive.
    client.onDidChangeState((ev) => {
        if (ev.newState === State.Starting) setStatus('starting');
        else if (ev.newState === State.Running) setStatus('ready');
        else if (ev.newState === State.Stopped) setStatus('stopped');
    });

    setStatus('starting');
    try {
        await client.start();
    } catch (err: any) {
        setStatus('error', String(err?.message ?? err));
        // Surface the exact failure so the user knows whether it's "binary
        // not found" vs. a protocol error vs. something else.
        const detail = err?.message ?? String(err);
        const action = await window.showErrorMessage(
            `Rask LSP failed to start (${detail}). Syntax highlighting still works.`,
            'Open Output',
            'Configure Path',
        );
        if (action === 'Open Output') {
            client?.outputChannel.show();
        } else if (action === 'Configure Path') {
            await commands.executeCommand(
                'workbench.action.openSettings',
                'rask.serverPath',
            );
        }
    }
}

async function stopClient(): Promise<void> {
    if (!client) return;
    try {
        await client.stop();
    } catch {
        // Ignore — we're tearing down anyway.
    }
    client = undefined;
}

async function restartClient(context: ExtensionContext): Promise<void> {
    await stopClient();
    await startClient(context);
}

function runInTerminal(command: string, filePath?: string): void {
    const editor = window.activeTextEditor;
    if (!filePath && (!editor || editor.document.languageId !== 'rask')) {
        window.showWarningMessage('Open a .rk file first.');
        return;
    }
    const target = filePath ?? editor!.document.fileName;
    const terminal = window.createTerminal('Rask');
    terminal.show();
    terminal.sendText(`${resolveRaskPath()} ${command} "${target}"`);
}

export async function activate(context: ExtensionContext): Promise<void> {
    statusBar = window.createStatusBarItem(StatusBarAlignment.Left, 100);
    statusBar.command = 'rask.restartServer';
    context.subscriptions.push(statusBar);

    await startClient(context);

    context.subscriptions.push(
        commands.registerCommand('rask.run', () => runInTerminal('run')),
        commands.registerCommand('rask.check', () => runInTerminal('check')),
        commands.registerCommand('rask.test', () => runInTerminal('test')),
        commands.registerCommand('rask.build', () => runInTerminal('build')),

        commands.registerCommand('rask.restartServer', () => restartClient(context)),

        commands.registerCommand('rask.showOutput', () => {
            client?.outputChannel.show();
        }),

        // The LSP provides formatting, but keep this legacy path as a fallback
        // for when the server isn't running.
        languages.registerDocumentFormattingEditProvider('rask', {
            provideDocumentFormattingEdits(document: TextDocument): Promise<TextEdit[]> {
                // If the LSP is running, its formatter wins — VS Code applies
                // the server's edits before calling this fallback.
                if (client && client.state === State.Running) {
                    return Promise.resolve([]);
                }
                return new Promise((resolve) => {
                    const proc = spawn(resolveRaskPath(), ['fmt']);
                    let stdout = '';
                    let stderr = '';
                    proc.stdout.on('data', (data: Buffer) => { stdout += data.toString(); });
                    proc.stderr.on('data', (data: Buffer) => { stderr += data.toString(); });
                    proc.on('error', (err: Error) => {
                        window.showWarningMessage(`rask fmt failed: ${err.message}`);
                        resolve([]);
                    });
                    proc.on('close', (code: number) => {
                        if (code !== 0) {
                            window.showWarningMessage(`rask fmt failed: ${stderr}`);
                            resolve([]);
                            return;
                        }
                        const fullRange = new Range(
                            document.positionAt(0),
                            document.positionAt(document.getText().length),
                        );
                        resolve([TextEdit.replace(fullRange, stdout)]);
                    });
                    proc.stdin.write(document.getText());
                    proc.stdin.end();
                });
            },
        }),
    );

    // Auto-restart when the user changes the server path or similar.
    context.subscriptions.push(
        workspace.onDidChangeConfiguration(async (e) => {
            if (e.affectsConfiguration('rask.serverPath')) {
                await restartClient(context);
            }
        }),
    );
}

export async function deactivate(): Promise<void> {
    await stopClient();
}
