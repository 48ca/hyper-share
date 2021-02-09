#!/bin/bash -ue

set -m # Enable jobs

YELLOW='\033[0;33m'
NC='\033[0m' # No Color

export PORT=12389
export DIR=$(mktemp -d)
export BOUNDARY="aaaaaaaaaaaaaaaaaaaa" # 20 a's

echo "Starting hypershare"

cargo build
cargo run -- -d $DIR -p $PORT -m "127.0.0.1" -u --headless | sed -e 's/^/ >>> hypershare: /g' &

sleep 1

set +e

pushd $(dirname ${BASH_SOURCE[0]}) > /dev/null

echo "Generating files..."
dd if=/dev/urandom of=$DIR/test_1m.img bs=1K count=1K 2>&1 > /dev/null
dd if=/dev/urandom of=$DIR/test_512m.img bs=1K count=512K 2>&1 > /dev/null
touch $DIR/test_0b.img
echo ":)" > $DIR/test_small.img
echo ":)" > $DIR/file\ with\ spaces\ and\ %s

function errored() {
    echo -e "${YELLOW}!!! Test errored${NC}"
}

echo -e "\n.......... GET Requests ..........."

echo "TEST: 1M file... "
templates/wget_get_request.sh test_1m.img || errored

echo "TEST: 512M file... "
templates/wget_get_request.sh test_512m.img || errored

echo "TEST: 0B file... "
templates/wget_get_request.sh test_0b.img || errored

echo "TEST: Small file... "
templates/wget_get_request.sh test_small.img || errored

echo -e "\n.... Well-Formed POST Requests (curl) ...."

echo "TEST: 1M file... "
templates/curl_post_request.sh test_1m.img || errored

echo "TEST: 512M file... "
templates/curl_post_request.sh test_512m.img || errored

echo "TEST: 0B file... "
templates/curl_post_request.sh test_0b.img || errored

echo "TEST: Small file... "
templates/curl_post_request.sh test_small.img || errored

echo "TEST: File with spaces... "
templates/curl_post_request.sh "file with spaces and %s" || errored

echo -e "\n.... Well-Formed POST Requests (custom) ...."

echo "TEST: 1M file... "
templates/wellformed_post_request.sh test_1m.img || errored

echo "TEST: 512M file... "
templates/wellformed_post_request.sh test_512m.img || errored

echo "TEST: 0B file... "
templates/wellformed_post_request.sh test_0b.img || errored

echo "TEST: Small file... "
templates/wellformed_post_request.sh test_small.img || errored

echo "TEST: Small file with expectation... "
templates/wellformed_post_request_with_continue.sh test_small.img || errored

echo -e "\n.... GET + POST Requests (curl/wget) ...."

echo "TEST: 1M file... "
templates/curl_wget_twoway.sh test_1m.img || errored

echo "TEST: 512M file... "
templates/curl_wget_twoway.sh test_512m.img || errored

echo "TEST: 0B file... "
templates/curl_wget_twoway.sh test_0b.img || errored

echo "TEST: Small file... "
templates/curl_wget_twoway.sh test_small.img || errored

echo "TEST: File with spaces... "
templates/curl_wget_twoway.sh "file with spaces and %s" || errored

echo -e "...................................\n"
echo "Killing hypershare and cleaning up"

rm $DIR/test_1m.img
rm $DIR/test_512m.img
rm $DIR/test_0b.img
rm $DIR/test_small.img
rm $DIR/file\ with\ spaces\ and\ %s

kill -2 %1

rm -r $DIR

popd > /dev/null
