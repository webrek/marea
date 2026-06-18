//! Binario dedicado `marea-lsp`: arranca el servidor de lenguaje y delega en
//! [`marea_lsp::run`]. Es el único punto que toca dependencias externas, por
//! diseño de empaque (no se agrega subcomando a `marea-cli`).

use lsp_server::Connection;

fn main() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    // Transporte por entrada/salida estándar: el editor habla con el servidor
    // por stdin/stdout. `io_threads` son los hilos de lectura/escritura.
    let (connection, io_threads) = Connection::stdio();
    marea_lsp::run(connection)?;
    // Espera a que los hilos de E/S terminen de drenar el canal antes de salir.
    io_threads.join()?;
    Ok(())
}
