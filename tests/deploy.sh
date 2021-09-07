#!/bin/bash

set -e

CHAIN=$1

DIR="$( cd "$( dirname "$0" )" && pwd )"
REPO_PATH="$( cd "$( dirname "$0" )" && cd ../ && pwd )"

LOG_DIR=$DIR/logs/${CHAIN}
mkdir -p $LOG_DIR

DATA_DIR=$DIR/data
mkdir -p $DATA_DIR

echo "Build node"
cargo build -p ${CHAIN}-bridge-node

EXECUTABLE=$REPO_PATH/target/release/${CHAIN}-bridge-node
index=100

if [[ "$CHAIN" == "millau" ]] ; then
  index=100
fi

if [[ "$CHAIN" == "rialto" ]] ; then
  index=200
fi

for validator in alice bob charlie dave eve ferdie
do
  echo "Purge $validator's \`db\`, \`network\`"
  rm -rf $DATA_DIR/$validator/chains/$CHAIN/db
  rm -rf $DATA_DIR/$validator/chains/$CHAIN/network

  echo "Firing ${CHAIN} Node ${validator}"
  ${EXECUTABLE} \
    --base-path $DATA_DIR/$validator \
    --$validator \
    --chain local \
    --port $((30333 + index)) \
    --ws-port $((9944 + index)) \
    --node-key 0000000000000000000000000000000000000000000000000000000000000$((1 + index)) \
    --unsafe-ws-external \
    --rpc-cors all &> $LOG_DIR/$validator.log &

  index=$((index + 1))
done
