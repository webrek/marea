//! `marea-lsp` — servidor de lenguaje (Language Server Protocol) de Marea.
//!
//! Implementación síncrona al estilo de rust-analyzer: usa `lsp-server` para el
//! transporte por stdio y `lsp-types` para el modelo de datos del protocolo.
//! Reúsa el front-end del compilador (`marea-syntax` y `marea-types`) sin
//! modificarlo; las dependencias externas quedan aisladas en este crate.
//!
//! La unidad de análisis es el PROGRAMA, no el archivo: [`programa`] resuelve el
//! grafo de `import` del documento abierto —leyendo de los buffers del editor
//! antes que del disco— y lo verifica entero con `check_program`. [`analysis`]
//! sigue siendo el análisis de UN módulo, que es lo que se usa cuando el
//! documento no importa nada.

pub mod analysis;
pub mod capabilities;
pub mod conversions;
pub mod documents;
pub mod features;
pub mod line_index;
pub mod programa;
pub mod server;

/// Punto de entrada del bucle del servidor sobre una [`lsp_server::Connection`]
/// ya construida (stdio en producción, en memoria en pruebas). Reexporta
/// [`server::run`].
pub use server::run;

/// Analiza una fuente Marea y devuelve su AST (si parsea) y sus diagnósticos.
/// Reexporta [`analysis::analyze`] para uso directo (pruebas, herramientas).
pub use analysis::analyze;
