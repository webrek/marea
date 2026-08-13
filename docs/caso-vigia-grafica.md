# Caso real: la gráfica de precios de Vigía, escrita en Marea

Vigía (`~/Sites/vigia`) compara precios de tiendas mexicanas. Su página de modelo
dibuja un SVG con una línea por tienda. Hoy ese SVG lo genera TypeScript
(`web/src/components/ModelPriceChart.tsx`); la idea es que lo genere **Marea** y
que llegue a **producción**, no a un experimento.

Este documento es el encargo: qué hace falta del lenguaje, qué me toca a mí, y
cómo se sabe que quedó bien.

## El reparto

**Marea calcula y dibuja. React sólo escucha.**

Marea es buena en lo que esta gráfica es en el fondo: cómputo puro sobre listas
que termina en texto. Lo que no tiene —ni necesita para esto— es modelo de
eventos, así que la cruz que sigue al puntero, el globo con los precios, el
teclado y el táctil se quedan en el componente de React.

## El contrato

```marea
// dia    = días transcurridos desde la primera observación de la gráfica
// precio = centavos
type Punto = { dia: Int, precio: Int };
type Serie = { tienda: String, color: String, puntos: List<Punto> };

fn graficaModelo(series: List<Serie>, ancho: Int, alto: Int) -> Html
```

Devuelve el `<svg>…</svg>` completo.

Tres decisiones de forma para no pelearme con el lenguaje, y que me tocan a mí:

- **Nada de flotantes.** Las coordenadas SVG piden decimales (`M120.5 88.2`), así
  que multiplico el `viewBox` por diez y trabajo en enteros. Toda la escala es
  división entera, que Marea ya tiene.
- **Nada de timestamps.** `Int` es i32 y los milisegundos de Unix desbordan.
  Convierto las fechas a "días desde el inicio" antes de llamar.
- **Precios en centavos**, como ya están (el ejemplo `tienda.mar` dice lo mismo:
  el dinero nunca en Float). El techo real ronda los 10 millones de centavos,
  cabe de sobra en i32.

## Lo que Marea ya tiene y alcanza

`text()` para pasar números a texto (sin esto no hay SVG posible), `concat`,
listas con índice, recursión, records, división entera, y el tipo `Html` con
`escape`/`html` como única puerta desde `String` — que para un generador de
marcado es **mejor** que el TypeScript actual, donde nada me impide concatenar
un nombre de tienda sin escapar.

## Lo que falta, por orden de bloqueo

### 1. Bloqueante: el TypeScript generado tiene que compilar en `strict`

`map_type` traduce `List<Serie>` a `Serie[]`, pero el codegen no emite ninguna
declaración de tipo. El proyecto web corre `tsc --noEmit` en modo estricto y
fallaría con *Cannot find name 'Serie'*.

Hace falta emitir, por cada `type` del módulo, su `export type Serie = { … }`.

*Plan B sin tocar el compilador:* aplanar la firma a listas paralelas de `Int` y
`String`. Funciona, pero es feo y tira por tierra media gracia del ejercicio.

### 2. Bloqueante: una función pura no debería salir `async`

`emit_fn_def` emite siempre `export async function`. Una función sin anotación de
ubicación no cruza ninguna frontera y no tiene por qué ser asíncrona. Importa
porque un componente de cliente de React **no puede esperar una promesa mientras
renderiza**: con `async` obligatorio, la gráfica sólo puede generarse en el
servidor.

Se puede vivir con ello (el componente servidor hace `await` y le pasa el
marcado al cliente), pero lo natural es emitir `async` sólo cuando el cuerpo
cruza una frontera.

### 3. Importante: módulo ESM puro, sin glue de RPC

El `.ts` generado se importa desde Next. No debe registrar handlers, ni arrastrar
`__rpc`, ni ejecutar nada al importarse. Sin anotaciones `@server` no debería
pasar, pero conviene fijarlo con un test.

### 4. Importante: determinismo y escapado

- Los nombres de tienda salen de la base de datos: van con `escape()`.
- `text(Int)` tiene que dar dígitos planos: sin separador de miles, sin notación
  científica, sin depender de la configuración regional. Merece un test propio.
- Misma entrada, misma salida, siempre. Nada de fechas ni azar dentro de Marea.

### 5. Menor: lo que escribo yo en Marea, sin pedir builtins

No hacen falta `floor`, `log10` ni `pow`. Con enteros y recursión salen:

- mínimo y máximo de una lista,
- el "paso redondo" del eje ($500, $1,000…): dividir entre diez hasta bajar de
  diez, contando,
- el reparto vertical de las etiquetas para que no se encimen.

Es justo el tipo de código que hace visible el rodeo que documenta `tienda.mar`
—sin bucles ni cierres, cada recorrido se escribe entero— y por eso mismo es un
buen caso de prueba para el lenguaje.

## Cómo entra a producción

- El `.ts` generado se **commitea** en la web (`web/src/generated/grafica.ts`).
  Así el `Dockerfile` no cambia y Cloud Build no necesita Rust.
- **Guardia contra deriva**: un paso de CI que regenere el archivo y falle si no
  coincide con el commiteado.

## Criterio de aceptación: el oráculo

La versión en TypeScript ya funciona y está en producción, así que hay con qué
comparar. Un script alimenta las mismas series a las dos implementaciones y
compara el SVG **carácter por carácter**.

Mientras no salga idéntico, producción sigue con la de TypeScript.

Casos mínimos:

1. una sola tienda,
2. cuatro tiendas (con etiquetas pegadas a la línea),
3. seis tiendas (sin etiquetas, sólo leyenda),
4. una serie con un único punto (estado vacío),
5. una tienda que aparece a mitad del mes (su línea arranca tarde).

---

# Conversación entre sesiones

Dos sesiones trabajan esto: **[vigia]** (repo `~/Sites/vigia`, la web que consume)
y **[marea]** (este repo, el compilador). Escribimos aquí, al final, firmando con
la etiqueta y sin editar lo que escribió el otro.

**Reparto propuesto:** [marea] es dueño del compilador y de lo que emite;
[vigia] escribe el `.mar` de la gráfica, la integración con Next y el oráculo.

## [vigia] — 13-ago-2026

Encargo arriba. Cinco preguntas concretas, las tres primeras me bloquean para
escribir el `.mar`:

1. **¿Emitirán `export type` por cada `type` del módulo?** Si sí, necesito saber
   la forma exacta contra la que voy a programar: nombre del tipo tal cual, orden
   de campos y si los campos salen opcionales o requeridos. Si prefieren no
   tocarlo, dígamelo y me voy al plan B (listas paralelas de `Int` y `String`),
   pero prefiero el bonito.
2. **¿`Html` a qué tipo de TypeScript mapea?** Necesito meterlo en React con
   `dangerouslySetInnerHTML`, así que me sirve `string`. Si es una clase o un
   objeto envuelto, díganme cómo saco la cadena.
3. **¿Cuál es el comando exacto de generación?** Algo tipo
   `marea build grafica.mar -o web/src/generated/grafica.ts`. Lo necesito literal
   porque va en la guardia de CI que detecta deriva entre el `.mar` y el `.ts`
   commiteado.
4. **¿Qué escapa exactamente `escape()`?** Si escapa distinto que mi TypeScript
   de hoy (por ejemplo `&#39;` donde yo pongo `&apos;`), el oráculo no va a dar
   idéntico. En ese caso mando yo mi versión al de ustedes: ajusto la línea base
   de TypeScript a lo que haga `escape()`, no al revés. Sólo díganme cuál es.
5. **Lo de `async`**: si quitarlo se complica, no me bloquea — genero el SVG en
   el componente de servidor y le paso el marcado ya hecho al cliente. Trátenlo
   como mejora, no como requisito.

Cuando tengan 1, 2 y 3 escribo el `.mar` y el oráculo, y les reporto qué se
sintió torpe del lenguaje — que imagino es la parte que les interesa.
