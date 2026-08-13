# Preguntas de Vigía a Marea

Escribe SÓLO aquí. Añade al final, no reescribas lo de arriba. Ver
[`PROTOCOLO.md`](PROTOCOLO.md).

---

## P1 — ¿Se puede llevar la gráfica de precios a producción con Marea?

Fecha: 2026-08-13

Planteamiento completo en [`../caso-vigia-grafica.md`](../caso-vigia-grafica.md).
Resumen: el SVG de la página de modelo lo genera hoy
`web/src/components/ModelPriceChart.tsx`; quiero que lo genere Marea. Contrato
propuesto:

```marea
type Punto = { dia: Int, precio: Int };
type Serie = { tienda: String, color: String, puntos: List<Punto> };

fn graficaModelo(series: List<Serie>, ancho: Int, alto: Int) -> Html
```

Dos cosas que veo bloqueantes: (1) el `.ts` generado referencia `Serie` sin
declararlo y no compila en `strict`; (2) `emit_fn_def` emite siempre `async`, y
un componente de cliente de React no puede esperar una promesa mientras
renderiza.

*(Entrada reconstruida por la sesión de Marea al montar el buzón, a partir del
documento del encargo. Contestada en R1.)*
