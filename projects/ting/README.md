<!-- id: ting.overview -->
<!-- status: proposed -->
<!-- summary: Ting -- real-time server-to-client multiplayer over UDP, testable under sim -->

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
│  1. Endpoint   acks, resends,        │  reliable · unreliable
│                ordering              │
├──────────────────────────────────────┤
│  0. Datagrams  UDP · sim network     │  where packets go
└──────────────────────────────────────┘
```

Each layer only knows the one below.

### 0. Datagrams

A UDP socket (`std.net/U1`). Under sim it's an in-process network where the seed picks some links to lose, repeat and reorder datagrams for the whole run (`sim/N1`).

### 1. Endpoint (built)

`endpoint.rk`. One socket, any number of peers, two kinds of message:

- **Reliable**: sent again in every packet to its peer until a packet carrying it is acknowledged, and handed over once, in the order it was sent.
- **Unreliable**: goes in one packet and is forgotten.

Both kinds share packets, so a lost chat line costs a resend and never holds up a position update. Every packet carries acks: the newest sequence number heard from the peer and a bit for each of the 32 before it.

```rask
mut ep = try Endpoint.bind("0.0.0.0:7777")
ensure ep.close()
ep.send(client, state_bytes, reliable: false)
try ep.flush()                       // once a tick
for m in try ep.poll() { ... }       // whatever arrived, without waiting
```

`reliability.rk` checks the promise under sim. With every link made bad, 200 runs deliver every reliable message once and in order, while a protocol that trusts UDP fails on its first seed.

Barebones on purpose: no encryption, no handshake, no splitting of a message too big for one packet, and no send-rate control yet.

### 2. Game (not built)

The server runs a fixed tick, from 10 Hz for a strategy game to 60 or more for a shooter.

**Inputs.** The client tags each input with its tick and sends it unreliable, with the last few inputs repeated in every packet, so one lost packet costs nothing.

**Snapshots.** Each tick the server builds one snapshot per client: what that client can see (the game's relevance function), as a delta against the last snapshot the client acknowledged.

**History.** The server keeps the last N ticks of state, for lag compensation ("was the target there in the frame the shooter saw?") and for checking a client's prediction.

The game supplies `simulate(state, inputs) -> state` for one tick, `relevance(state, client)`, and optional prediction and lag-compensation hooks. If `simulate` is written in Raido, the client can run the same bytecode to predict and the server can re-run any client's prediction bit for bit (`gdl.extensions` describes the same model). Rules in Rask work too, without that check: Rask floats are only deterministic within one binary (`determinism/D12`).

## Why not QUIC

I wanted QUIC for its encryption and for browsers (WebTransport). The ngtcp2 1.x source I could get comes paired with aws-lc, a BoringSSL fork, and BoringSSL mixes the CPU's RDRAND instruction into its randomness when the CPU has it. Sim can only intercept libc calls (`sim/B7`), and a handshake that doesn't replay breaks the one thing Ting is for. The packaged alternative, ngtcp2 0.12 with GnuTLS, is a pre-1.0 API that newer distributions have already dropped. So the endpoint is our own, and encryption will come from libsodium, which gets its randomness from `getrandom`. QUIC can still arrive later as another layer 0 for browser clients.

## Clients

Rask can't draw, so real clients will usually be a game engine or a browser. The wire format is specified so any language can implement it. Bots, load tests and sim tests are Rask clients, and they're how the stack gets tested.

## Not Leden

[Leden](../leden/README.md) is a capability protocol for federated endpoints over reliable streams. Ting is one server, many clients, packets that may get lost. They share no code and Ting doesn't change Leden.

## Next

1. Send-rate control and splitting large reliable messages.
2. Encryption and a connect handshake (libsodium; a matchmaker-signed connect ticket).
3. The game layer, with a small example game.
