// Extensión de VSCode para Marea: arranca el servidor de lenguaje 'marea-lsp'
// y conecta VSCode a él por stdio. Toda la inteligencia (diagnósticos,
// símbolos, completado, definición, hover) vive en el servidor en Rust; esta
// extensión es solo el cliente.

import { workspace, ExtensionContext, window } from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export function activate(context: ExtensionContext): void {
  const config = workspace.getConfiguration("marea");
  const serverPath = config.get<string>("serverPath") || "marea-lsp";

  // El mismo binario para ejecución normal y depuración: habla LSP por stdio.
  const serverOptions: ServerOptions = {
    run: { command: serverPath, transport: TransportKind.stdio },
    debug: { command: serverPath, transport: TransportKind.stdio },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "marea" }],
    synchronize: {
      fileEvents: workspace.createFileSystemWatcher("**/*.mar"),
    },
  };

  client = new LanguageClient(
    "marea",
    "Servidor de lenguaje Marea",
    serverOptions,
    clientOptions
  );

  client.start().catch((err) => {
    window.showErrorMessage(
      `No se pudo iniciar 'marea-lsp' (${serverPath}). ` +
        `Compílalo con 'cargo build -p marea-lsp' y ajusta 'marea.serverPath'. Detalle: ${err}`
    );
  });

  context.subscriptions.push({ dispose: () => void client?.stop() });
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
