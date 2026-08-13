# Gramática de Marea (v0)

Notación EBNF informal. `*` = cero o más, `?` = opcional, `|` = alternativa.
Refleja exactamente lo que el parser de v0 acepta hoy
(`crates/marea-syntax/src/parser.rs`).

```ebnf
module      = item* ;

item        = fn_decl
            | type_decl
            | let_stmt
            | store_decl ;

(* --- ubicación: solo aplica a funciones --- *)
location    = "@" ( "server" | "client" | "edge" ) ;

fn_decl     = location? "fn" IDENT "(" params? ")" ( "->" type )? block ;
params      = param ( "," param )* ","? ;
param       = IDENT ":" type ;

type_decl   = "type" IDENT "=" type ";" ;

(* --- el estado del servidor: un solo tipo de elemento por módulo --- *)
store_decl  = "store" IDENT ":" type ";" ;

(* --- tipos: el '|' construye uniones --- *)
type        = type_primary ( "|" type_primary )* ;
type_primary= IDENT ( "<" type ( "," type )* ">" )?
            | record_type ;
record_type = "{" ( field_def ( "," field_def )* ","? )? "}" ;
field_def   = IDENT ":" type ;

(* --- bloques y sentencias --- *)
block       = "{" stmt* "}" ;
stmt        = let_stmt
            | assign
            | effect_stmt
            | "return" expr? ";"
            | expr ( ";" )? ;        (* ';' obligatorio salvo if/match *)

let_stmt    = ( "let" | "reactive" ) "mut"? IDENT ( ":" type )? "=" expr ";" ;
assign      = IDENT "=" expr ";" ;
effect_stmt = "effect" block ;

(* --- expresiones, por precedencia (menor a mayor) --- *)
expr        = or_expr ;
or_expr     = and_expr  ( "||" and_expr )* ;
and_expr    = eq_expr   ( "&&" eq_expr )* ;
eq_expr     = cmp_expr  ( ( "==" | "!=" ) cmp_expr )* ;
cmp_expr    = add_expr  ( ( "<" | ">" | "<=" | ">=" ) add_expr )* ;
add_expr    = mul_expr  ( ( "+" | "-" ) mul_expr )* ;
mul_expr    = unary     ( ( "*" | "/" | "%" ) unary )* ;
unary       = ( "-" | "!" ) unary
            | postfix ;
postfix     = primary ( call | member | index )* ;
call        = "(" ( expr ( "," expr )* ","? )? ")" ;
member      = "." IDENT ;
index       = "[" expr "]" ;

primary     = INT | FLOAT | STRING | BOOL | IDENT
            | record_lit
            | list_lit
            | "(" expr ")"
            | if_expr
            | match_expr ;

record_lit  = IDENT "{" ( field_init ( "," field_init )* ","? )? "}" ;
field_init  = IDENT ":" expr ;
list_lit    = "[" ( expr ( "," expr )* ","? )? "]" ;

if_expr     = "if" expr block ( "else" ( if_expr | block ) )? ;
match_expr  = "match" expr "{" ( arm "," )* arm? "}" ;
arm         = pattern "=>" expr ;
pattern     = "_" | IDENT | INT | BOOL | STRING ;
```

## Notas de diseño (v0)

- **Asociatividad**: todos los operadores binarios son asociativos por la
  izquierda. La precedencia está codificada en el parser Pratt
  (`parse_bin_expr`), de menor (`||`) a mayor (`* / %`).
- **`reactive` vs `let`**: misma forma sintáctica; el flag `reactive` viaja en el
  AST para que las fases posteriores construyan el grafo de dependencias. Un
  `let`/`reactive` de **nivel superior** es un item: el estado del módulo.
- **`effect`**: solo existe como sentencia (no como item). Su bloque se
  re-ejecuta cuando cambia alguna de las variables reactivas que leyó.
- **Asignación**: la parte izquierda es un identificador simple. `x.campo = e` y
  `xs[i] = e` **no** se parsean todavía. Se distingue de `==` mirando el token
  siguiente al identificador: solo un `=` suelto abre una asignación.
- **`store nombre: T;`**: declara un almacén del servidor con nombre. Un módulo
  puede declarar varios; el nombre se pasa como primer argumento y tipa los
  builtins de CRUD (`save`, `all`, `update`, `remove`). Es un item, y no
  admite atributo de ubicación.
- **Tipos unión**: `User | NotFound` es la base de "errores como valores". El
  `match` es la forma natural de consumirlos.
- **Tipos registro**: `{ autor: String, likes: Int }` es estructural y puede ir
  donde vaya un tipo (alias de `type`, parámetro, retorno, `store`). Los campos
  repetidos son un error de sintaxis, igual que en el literal.
- **Ambigüedad `Ident {`**: un identificador seguido de `{` es un literal de
  registro, **salvo** en la condición de un `if` o el escrutinio de un `match`,
  donde ese `{` abre el bloque. Dentro de contextos delimitados —`( )`, `[ ]`,
  argumentos de llamada, valor de un campo y cuerpo de una rama de `match`— vuelve
  a leerse como literal de registro.
- **`<` y `>` de genéricos**: se lexean como tokens sueltos, así que
  `List<Map<K, V>>` funciona sin un operador de shift que estorbe. Ojo: en la
  lista de argumentos de tipo **no** se admite coma final (sí en `( )`, `[ ]`,
  `{ }` y en los parámetros).
- **Coma final**: permitida en parámetros, argumentos de llamada, campos (tipo y
  literal), elementos de lista y ramas de `match`.
- **Profundidad máxima**: el parser de expresiones corta a 128 niveles de
  anidamiento (`MAX_DEPTH`) y devuelve un error ordinario en vez de desbordar la
  pila.
- **Recuperación de errores**: `parse_module_recovering` (la que usan `check`,
  los `build*` y el LSP) reporta **todos** los errores de sintaxis, saltando al
  siguiente item probable (`@`, `fn`, `type`, `let`, `reactive`).

## Pendiente (fuera de v0)

Cierres/lambdas, `import`, operador de propagación de errores, genéricos en
funciones, patrones de registro/destructuring, y asignación a campos o a
elementos de una lista.

> `import` **se lexea** como palabra clave, pero no hay ninguna producción que lo
> acepte: hoy un `import` en el fuente es un error de sintaxis.
