#!/usr/bin/env bash
# postgres:16-alpine, mysql:8.4, mariadb:11.4 컨테이너를 빈 포트에 띄우고 KILN_TEST_PG_URL / KILN_TEST_MYSQL_URL 을
# 설정한 뒤 kiln-db 테스트를 실행한다. 종료 시 컨테이너를 지운다.
# 사용: crates/kiln-db/tests/run_docker_tests.sh [cargo test 추가 인자]
set -euo pipefail

free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'; }

PG_PORT=$(free_port)
MY_PORT=$(free_port)
MA_PORT=$(free_port)
PG_NAME="kiln-db-test-pg-$$"
MY_NAME="kiln-db-test-mysql-$$"
MA_NAME="kiln-db-test-maria-$$"

cleanup() { docker rm -f "$PG_NAME" "$MY_NAME" "$MA_NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

docker run -d --rm --name "$PG_NAME" -e POSTGRES_PASSWORD=kiln -e POSTGRES_DB=kiln \
  -p "127.0.0.1:${PG_PORT}:5432" postgres:16-alpine >/dev/null
docker run -d --rm --name "$MY_NAME" -e MYSQL_ROOT_PASSWORD=kiln -e MYSQL_DATABASE=kiln \
  -p "127.0.0.1:${MY_PORT}:3306" mysql:8.4 >/dev/null

docker run -d --rm --name "$MA_NAME" -e MARIADB_ROOT_PASSWORD=kiln -e MARIADB_DATABASE=kiln \
  -p "127.0.0.1:${MA_PORT}:3306" mariadb:11.4 >/dev/null

echo "waiting for postgres on ${PG_PORT} ..."
for _ in $(seq 1 120); do
  docker exec "$PG_NAME" pg_isready -U postgres -d kiln >/dev/null 2>&1 && break
  sleep 1
done
echo "waiting for mysql on ${MY_PORT} ..."
for _ in $(seq 1 180); do
  docker exec "$MY_NAME" mysql -uroot -pkiln -h127.0.0.1 -e 'SELECT 1' kiln >/dev/null 2>&1 && break
  sleep 1
done

echo "waiting for mariadb on ${MA_PORT} ..."
for _ in $(seq 1 180); do
  docker exec "$MA_NAME" mariadb -uroot -pkiln -h127.0.0.1 -e 'SELECT 1' kiln >/dev/null 2>&1 && break
  sleep 1
done

export KILN_TEST_MARIA_URL="mysql://root:kiln@127.0.0.1:${MA_PORT}/kiln?ssl-mode=disabled"
export KILN_TEST_PG_URL="postgres://postgres:kiln@127.0.0.1:${PG_PORT}/kiln?sslmode=disable"
export KILN_TEST_MYSQL_URL="mysql://root:kiln@127.0.0.1:${MY_PORT}/kiln?ssl-mode=disabled"
echo "KILN_TEST_PG_URL=$KILN_TEST_PG_URL"
echo "KILN_TEST_MYSQL_URL=$KILN_TEST_MYSQL_URL"

cd "$(dirname "$0")/../../.."
cargo test --locked -p kiln-db --test pg_backend --test mysql_backend --test schema_changes "$@"
cargo test --locked -p kiln-db --test sql_completion -- --test-threads=1
