#!/bin/bash -ue

pushd $(dirname ${BASH_SOURCE[0]}) > /dev/null

./run_tests.sh 2>/dev/null

popd > /dev/null
