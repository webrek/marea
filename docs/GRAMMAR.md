# Gramática de Marea (v0)

Notación EBNF informal. `*` = cero o más, `?` = opcional, `|` = alternativa.
Refleja exactamente lo que el parser de v0 acepta hoy.

```ebnf
module      = item* ;

item        = fn_decl
            | type_decl
            | let_stmt ;

(* --- ubicación: solo aplica a funciones --- *)
location    = "@" ( "server" | "client" | "edge" ) ;

fn_decl     = location? "fn" IDENT "(" params? ")" ( "->" type )? block ;
params      = param ( "," param )* ","? ;
param       = IDENT ":" type ;

type_decl   = "type" IDENT "=" type ";" ;

(* --- tipos: el '|' construye uniones --- *)
type        = type_primary ( "|" type_primary )* ;
type_primary= IDENT ( "<" type ( "," type )* ">" )? ;

(* --- bloques y sentencias --- *)
block       = "{" stmt* "}" ;
stmt        = let_stmt
            | "return" expr? ";"
            | expr ( ";" )? ;        (* ';' obligatorio salvo if/match *)

let_stmt    = ( "let" | "reactive" ) "mut"? IDENT ( ":" type )? "=" expr ";" ;

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
postfix     = primary ( call | member )* ;
call        = "(" ( expr ( "," expr )* ","? )? ")" ;
member      = "." IDENT ;

primary     = INT | FLOAT | STRING | BOOL | IDENT
            | "(" expr ")"
            | if_expr
            | match_expr ;

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
  AST para que las fases posteriores construyan el grafo de dependencias.
- **Tipos unión**: `User | NotFound` es la base de "errores como valores". El
  `match` es la forma natural de consumirlos.
- **`<` y `>` de genéricos**: se lexean como tokens sueltos, así que
  `List<Map<K, V>>` funciona sin un operador de shift que estorbe.

## Pendiente (fuera de v0)

Cierres/lambdas, literales de registro/lista, `import`, operador de propagación
de errores, genéricos en funciones, y la semántica (tipos, nombres, runtime).
