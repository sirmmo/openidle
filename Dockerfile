# The server, with the games and the browser player:
#   docker build -t openidle . && docker run -p 8080:8080 -v openidle-data:/data openidle
FROM rust:1.95 AS build
RUN rustup target add wasm32-unknown-unknown
WORKDIR /src
COPY . .
RUN cargo build --release -p openidle-server -p openidle-cli \
 && cargo build --profile wasm --target wasm32-unknown-unknown -p openidle-web \
 && cp target/wasm32-unknown-unknown/wasm/openidle_web.wasm web/openidle.wasm \
 && mkdir -p web/games \
 && for g in games/*/; do ./target/release/openidle bundle "$g" -o "web/games/$(basename "$g").json"; done

FROM debian:bookworm-slim
COPY --from=build /src/target/release/openidle-server /usr/local/bin/
COPY --from=build /src/games /srv/games
COPY --from=build /src/web /srv/web
ENV OI_GAMES=/srv/games OI_SITE=/srv/web OI_DATA=/data OI_ADDR=0.0.0.0:8080
EXPOSE 8080
CMD ["openidle-server"]
