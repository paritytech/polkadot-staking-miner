# Polkadot Staking Miner

[GitHub](https://github.com/paritytech/polkadot-staking-miner)

Formerly known as `staking-miner-v2` historical images versions are available in the [hub.docker.com](https://hub.docker.com/r/paritytech/staking-miner-v2)

## Using Pre-built Docker Image

### Pull the image from Docker Hub

```bash
docker pull polkadot-staking-miner
```

Or if using a specific registry:

```bash
docker pull <registry>/polkadot-staking-miner:<tag>
```

### Run prediction with custom file

```bash
sudo docker run --rm \
  -v "$(pwd):/workspace" \
  polkadot-staking-miner \
  --uri wss://westend-asset-hub-rpc.polkadot.io \
  predict --custom-file custom.json --desired-validators 1
```

### Run basic prediction

```bash
docker run --rm \
  -v "$(pwd):/workspace" \
  polkadot-staking-miner \
  --uri wss://westend-asset-hub-rpc.polkadot.io \
  predict
```

## Building Locally

If you need to build the image locally:

```bash
docker build -t polkadot-staking-miner .
```



### Pull the image from Docker Hub


sudo docker pull antiers/polkadot-staking-miner:latest

### for custom validators

sudo docker run --rm -v "$(pwd):/workspace" ant/polkadot-staking-miner:latest   --uri wss://westend-asset-hub-rpc.polkadot.io   predict --custom-file custom.json --desired-validators 1


### for basic prediction

sudo docker run --rm -v "$(pwd):/workspace" antiers/polkadot-staking-miner:latest   --uri wss://westend-asset-hub-rpc.polkadot.io   predict