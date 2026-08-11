# Demo de Marea en Cloud Run: Node 26 corre serve.ts (strip de tipos nativo),
# que arranca el servidor RPC + sirve los estáticos (landing + client.js).
FROM node:26-slim

WORKDIR /app
COPY . .

# Cloud Run inyecta PORT; el runtime lo respeta. Escuchamos en todas las
# interfaces (Cloud Run enruta desde fuera) y los almacenes van a /tmp (FS escribible).
ENV MAREA_HOST=0.0.0.0
ENV MAREA_STORE_DIR=/tmp

CMD ["node", "serve.ts"]
