# Canal entre la sesión de Vigía y la sesión de Marea

Vigía quiere generar su gráfica de precios con Marea. Vigía descubre lo que le
falta al lenguaje; Marea lo implementa. Esto es el buzón entre las dos, para que
no haga falta que Victor copie y pegue en medio.

## La regla, que es una sola

**Cada sesión escribe SÓLO en su archivo.** Nunca en el del otro.

| Archivo | Quién escribe | Quién lee |
|---|---|---|
| `PREGUNTAS.md` | la sesión de **Vigía** | Marea |
| `RESPUESTAS.md` | la sesión de **Marea** | Vigía |

Los dos son **append-only**: se añade al final, no se reescribe lo de arriba.
Archivos disjuntos + solo añadir = dos sesiones escribiendo a la vez sin
pisarse. Es lo mismo que hace que el reparto con `claude-duo` no choque.

## Formato

Una entrada por bloque, numerada y correlativa. La respuesta cita el número:

```markdown
## P3 — El TS generado no compila en strict
Fecha: 2026-08-13
`map_type` emite `Serie[]` pero `Serie` no se declara en ninguna parte.
¿Se puede emitir `export type`? Si no, sigo con la firma plana.
```

```markdown
## R3 — Sí, pero no lo necesitas todavía
Los tipos de RETORNO no se emiten, sólo los de parámetro; con parámetros
`Int`/`String`/`List<Int>`/`List<String>` el problema desaparece. Emitir
`export type` queda apuntado, no te bloquea.
```

Si una pregunta ya no aplica, se contesta igual diciendo eso: **nada se borra**,
porque el hilo entero es el registro de por qué el lenguaje acabó como acabó.

## Qué NO va aquí

- **Nadie commitea este directorio.** Si las dos sesiones commitean, vuelven los
  conflictos que este montaje existe para evitar. La sesión de Marea lo commitea
  de vez en cuando, cuando el hilo esté quieto.
- Peticiones vagas. "Necesito genéricos" no se puede contestar; "necesito pasar
  `List<Serie>` como parámetro y el `.ts` no declara `Serie`" sí. Lo que sirve es
  el caso concreto que te estrelló, con el `.mar` que lo produce.

## Estado del encargo

El planteamiento está en [`../caso-vigia-grafica.md`](../caso-vigia-grafica.md).
Ese documento es de Vigía; Marea no lo edita, comenta aquí.
