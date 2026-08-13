//! Los builtins **síncronos**, en un solo sitio.
//!
//! `marea-types` necesita saber si una llamada es síncrona para no exigirle un
//! contexto async, y `marea-codegen` lo necesita para decidir si emite `await`.
//! Las dos listas tienen que coincidir exactamente: si a una le sobra un nombre
//! se genera un `await` en un contexto síncrono, y si le falta se rechaza código
//! válido. Vivían duplicadas en tres sitios y había que sincronizarlas a mano
//! —dos de las copias ya habían acumulado nombres repetidos—, así que la lista
//! canónica es esta y los dos crates la consumen desde aquí. Es el crate base
//! del que ambos dependen.

/// Builtins que NO tocan la red ni la base de datos: cómputo puro sobre valores
/// que ya se tienen. Todo lo demás (estado, RPC) es asíncrono.
pub const SINCRONOS: &[&str] = &[
    // Salida y composición de texto/marcado.
    "print",
    "concat",
    "render",
    "len",
    "text",
    "escape",
    "html",
    // Listas y cadenas.
    "append",
    "contains",
    "lower",
    // Lectura de JSON por ruta: puro sobre un texto ya recibido.
    "jsonText",
    "jsonInt",
    "jsonFloat",
    "jsonLen",
];

/// `true` si el builtin es síncrono. Un nombre que no sea builtin da `false`,
/// que es lo correcto: una función del usuario puede cruzar la frontera.
pub fn es_sincrono(name: &str) -> bool {
    SINCRONOS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un duplicado no cambia el comportamiento, pero delata que la lista se
    /// edita a ojo: es exactamente el descuido que la hacía divergir cuando
    /// estaba copiada en tres archivos.
    #[test]
    fn la_lista_no_tiene_duplicados() {
        let mut vistos = std::collections::HashSet::new();
        for n in SINCRONOS {
            assert!(vistos.insert(*n), "'{n}' está repetido en SINCRONOS");
        }
    }

    #[test]
    fn los_builtins_de_estado_no_son_sincronos() {
        for n in ["save", "all", "update", "remove", "fetch", "post"] {
            assert!(!es_sincrono(n), "'{n}' pega a BD o red: debe ser async");
        }
    }
}
