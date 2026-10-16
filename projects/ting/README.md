<!-- id: ting.overview -->
<!-- status: proposed -->
<!-- summary: Ting -- real-time server-to-client multiplayer over QUIC, testable under sim -->

# Ting

Independent project. A real-time multiplayer stack for one authoritative server and many clients. Shooters, fighting games, RTS, MMO-style worlds: one core, tuned per game, so you don't pick a netcode before you pick a game.

The name is the Norse assembly, the place people met.

## What's hard, and what Ting does about it

Latency can't be removed, only hidden, and every way of hiding it makes the client and the server disagree for a while. That's game design and stays with the game. What Ting takes off the game's hands is everything underneath:

| Problem | Ting |
|---------|------|
| TCP stalls everything behind one lost packet | Each message picks its delivery: reliable, or lost-is-fine |
| Bandwidth | Snapshots are deltas against what the client last acknowledged, filtered by what the client can see |
| Prediction, interpolation, rewind | The server keeps tick history. The client hooks get the inputs and states they need |
| Desync and "it only happens at 200 ms with one dropped packet" | The whole stack runs under `rask test --sim`, with packet loss, delay and reordering from the seed |
| Cheating | The server is authoritative. Rules written in Raido run identically on the client, so the server can re-run a client's prediction and check it |

The last two are why this is a Rask project and not another netcode library. Game networking bugs are famously unreproducible. Under sim, a failing seed reproduces the network conditions along with the schedule.

## Layers

```
┌──────────────────────────────────────┐
│  3. Game       tick loop, snapshots, │  game supplies simulate, relevance,
│                inputs, history       │  prediction hooks
├──────────────────────────────────────┤
│  2. Channels   Reliable · Unreliable │  what a message needs
│                · Latest              │
├──────────────────────────────────────┤
│  1. QUIC       handshake, crypto,    │  streams + datagrams (RFC 9221)
│                acks, send rate       │
├──────────────────────────────────────┤
│  0. Datagrams  UDP · sim network     │  where packets go
└──────────────────────────────────────┘
```

Each layer only knows the one below. The game layer never sees QUIC; it says what a message needs and the channel layer provides it.

### 0. Datagrams

Send a packet, receive a packet. The real implementation is a UDP socket (`std.net/U1`). Under sim it's an in-process network where each link draws its behaviour from the seed: a delay and jitter, a loss rate, reordering, duplication. As with sim's other faults (`sim/F4`), the seed decides per link whether that link is bad for this run, so most runs are clean and some are awful, and the report says which link was which.

### 1. QUIC

I use QUIC rather than building reliability over UDP myself. It already does the hard parts: a handshake that can't be used to flood a third party, TLS 1.3, acknowledgements, send-rate control, connections that survive the client's IP changing. It also gives us browsers, through WebTransport.

The implementation is a C library with no I/O of its own (ngtcp2 or quiche): Ting hands it packets and the current time, and it hands back packets to send. That is what makes it testable. Sim drives it from the simulated network and the virtual clock. The TLS code underneath asks for random numbers and memory directly, which is why sim has to intercept C's clock, random and `malloc` first (`sim/B7`). Until that's built a QUIC handshake doesn't replay.

### 2. Channels

Three kinds, picked per message type:

| Kind | QUIC carries it as | Use |
|------|-------------------|-----|
| **Reliable** | One stream per channel | Chat, inventory, joins and leaves, anything that must arrive. Separate channels don't wait on each other |
| **Unreliable** | A datagram | Fire-and-forget: sounds, effects, redundant input |
| **Latest** | A datagram with a per-key sequence number | State where only the newest value matters. The receiver drops anything older than what it has |

A datagram has to fit in one packet (about 1200 bytes). A bigger Unreliable or Latest message is an error, not something Ting quietly splits. Anything that big belongs on a Reliable channel.

### 3. Game

The server runs a fixed tick. The tick rate is configuration, from 10 Hz for a strategy game to 60 or more for a shooter.

**Inputs.** The client tags each input with the tick it's for and sends it on an Unreliable channel. Every packet carries the last few inputs too, so one lost packet costs nothing. The server applies inputs at their tick. A late input is applied at the current tick or dropped, and the game decides which.

**Snapshots.** Each tick the server builds one snapshot per client: what that client can see (the game's relevance function), encoded as a delta against the last snapshot the client acknowledged. Acknowledgements ride back on input packets. A client that missed a few snapshots still decodes the next one, because the server only ever deltas against snapshots it knows arrived.

**History.** The server keeps the last N ticks of state. That's what lag compensation rewinds into ("was the target there in the frame the shooter saw?"), and what the game uses to check a client's prediction.

**What the game supplies:**

- `simulate(state, inputs) -> state`, one tick
- `relevance(state, client) -> what that client sees`
- optional prediction and reconciliation on the client, and lag compensation on the server

If `simulate` is written in Raido, the client can run the same bytecode to predict, and the server can re-run any client's prediction from the same inputs and get a bit-identical answer. That's both desync detection and anti-cheat (`gdl.extensions`, client scripts, describes the same model). Rules written in Rask work too, without that check, because Rask floats are only deterministic within one binary (`determinism/D12`).

## Clients

Rask can't draw, so real clients will usually be a game engine or a browser. The wire format is specified so any language can implement it, and the client half of the channel and game layers ships as a C library.

A Rask client exists from the start anyway. Bots, load tests and sim tests are all Rask clients, and they're how the stack gets tested.

## Not Leden

[Leden](../leden/README.md) is a capability protocol for federated endpoints over reliable streams. Ting is one server, many clients, packets that may get lost. They share no code and Ting doesn't change Leden. Ting borrows one idea: a matchmaker hands the client a signed ticket for one specific server, which is a capability in all but name.

## Order of work

1. Sim intercepts C's clock, random and `malloc` (`sim/B7`). Any linked C library needs this, not just QUIC.
2. UDP in the stdlib and runtime (`std.net/U1`–`U3`), and the simulated datagram network with per-link loss, delay and reordering.
3. QUIC binding, driven by Ting's own socket and clock.
4. Channels.
5. Game layer, with a small example game and a sim test that fails on a dropped packet and replays from its seed.

## Open

- Wire encoding for snapshots and deltas. Probably the Encode/Decode derive (`std.encoding`) with a bit-packed format, but unmeasured.
- Whether a fixed-point Raido interpreter is fast enough for a client to re-simulate several ticks per frame for many entities. Needs a benchmark before the game layer leans on it.
- Rollback for fighting games is the game layer plus save/restore of state. Ting provides the history and inputs; whether it should also provide the resimulate loop is open.
- Browser support depends on WebTransport, which needs HTTP/3 on top of QUIC.
