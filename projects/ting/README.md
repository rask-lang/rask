<!-- id: ting.overview -->
<!-- status: proposed -->
<!-- summary: Ting -- real-time server-to-client multiplayer over QUIC (ngtcp2), testable under sim -->

# Ting

Independent project. A real-time multiplayer stack for one authoritative server and many clients. Shooters, fighting games, RTS, MMO-style worlds: one core, tuned per game, so you don't pick a netcode before you pick a game.

The name is the Norse assembly, the place people met.

## What's hard, and what Ting does about it

Latency can't be removed, only hidden, and every way of hiding it makes the client and the server disagree for a while. That's game design and stays with the game. What Ting takes off the game's hands is everything underneath:

| Problem | Ting |
|---------|------|
| TCP stalls everything behind one lost packet | Each message picks reliable or unreliable, and both ride the same UDP packets |
| Bandwidth | Snapshots are deltas against what the client last acknowledged, filtered by what the client can see |
| Prediction, interpolation, rewind | The server keeps tick history. The client hooks get the inputs and states they need |
| Desync and "it only happens at 200 ms with one dropped packet" | The whole stack runs under `rask test --sim`, with packet loss, delay and reordering from the seed |
| Cheating | The server is authoritative. Rules written in Raido run identically on the client, so the server can re-run a client's prediction and check it |

The last two are why this is a Rask project and not another netcode library. Game networking bugs are famously unreproducible. Under sim, a failing seed reproduces the network conditions along with the schedule.

## Layers

```
┌──────────────────────────────────────┐
│  2. Game       tick loop, snapshots, │  game supplies simulate, relevance,
│                inputs, history       │  prediction hooks
├──────────────────────────────────────┤
│  1. QUIC       ngtcp2: acks, loss    │  reliable · unreliable
│                recovery, congestion  │
├──────────────────────────────────────┤
│  0. Datagrams  UDP · sim network     │  where packets go
└──────────────────────────────────────┘
```

Each layer only knows the one below.

### 0. Datagrams

A UDP socket (`std.net/U1`). Under sim it's an in-process network where the seed picks some links to lose, repeat and reorder datagrams for the whole run (`sim/N1`).

### 1. QUIC (built, barebones)

[ngtcp2](vendor/ngtcp2/README.md)'s core library, vendored, does the transport: acks, loss recovery, congestion and flow control, streams and datagrams. `quic/ting_quic.c` drives it, and `endpoint.rk` moves packets between it and the socket once a tick:

```rask
mut server = try Endpoint.listen("0.0.0.0:7777")
ensure server.close()
try server.send(client, state_bytes, reliable: false)
try server.flush()                       // once a tick
for m in try server.poll() { ... }       // whatever arrived, without waiting
```

A reliable message rides one QUIC stream per direction, arriving once and in order. An unreliable one is a QUIC DATAGRAM frame and may not arrive at all.

**Barebones means no TLS.** ngtcp2 leaves cryptography to callbacks, and here they're empty on purpose: a null cipher, a zero header mask, and a four-message fake handshake that only swaps transport parameters. Nothing is encrypted or authenticated. It also means nothing in the stack reads the clock or randomness except through libc, which sim answers from the seed (`sim/B7`). A TLS library is where determinism would break: BoringSSL and its forks mix the CPU's RDRAND instruction into their randomness, and sim can't intercept an instruction.

`reliability.rk` checks the promise under sim. With every link made bad, 150 runs deliver every reliable message once and in order, and the same seed replays to the same trace.

Native only: `extern "C"` needs a C ABI, which the interpreter doesn't have.

### 2. Game (not built)

The server runs a fixed tick, from 10 Hz for a strategy game to 60 or more for a shooter.

**Inputs.** The client tags each input with its tick and sends it unreliable, with the last few inputs repeated in every packet, so one lost packet costs nothing.

**Snapshots.** Each tick the server builds one snapshot per client: what that client can see (the game's relevance function), as a delta against the last snapshot the client acknowledged.

**History.** The server keeps the last N ticks of state, for lag compensation ("was the target there in the frame the shooter saw?") and for checking a client's prediction.

The game supplies `simulate(state, inputs) -> state` for one tick, `relevance(state, client)`, and optional prediction and lag-compensation hooks. If `simulate` is written in Raido, the client can run the same bytecode to predict and the server can re-run any client's prediction bit for bit (`gdl.extensions` describes the same model). Rules in Rask work too, without that check: Rask floats are only deterministic within one binary (`determinism/D12`).

## Clients

Rask can't draw, so real clients will usually be a game engine or a browser. The wire format is specified so any language can implement it. Bots, load tests and sim tests are Rask clients, and they're how the stack gets tested.

## Not Leden

[Leden](../leden/README.md) is a capability protocol for federated endpoints over reliable streams. Ting is one server, many clients, packets that may get lost. They share no code and Ting doesn't change Leden.

## Next

1. Real TLS behind the same callbacks, with its randomness made to replay under sim.
2. Connection lifetime: idle timeouts surfaced to the game, closing, a client that reconnects.
3. The game layer, with a small example game.
