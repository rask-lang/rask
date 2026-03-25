<!-- id: allgard.trust -->
<!-- status: proposed -->
<!-- summary: Adversarial trust model — how domains build, verify, and lose trust -->

# Trust

How domains build, verify, and lose trust. Modeled on how humans actually trust each other — not on cryptoeconomics.

The [Conservation Laws](CONSERVATION.md) define what's valid. This spec defines how domains decide *who to believe*.

## The Human Trust Model

I looked at proof-of-work, proof-of-stake, token deposits, slashing conditions. They all solve the wrong problem. They try to make dishonesty expensive through money. But real trust doesn't work that way.

Real trust works like this:

1. Someone I trust introduces you
2. We start with small interactions
3. You build a track record over time
4. If you betray me, I cut you off — and I tell the person who introduced you
5. That person's judgment is now in question too

No deposits. No tokens. The stake is your reputation — the thing that took months or years to build. Losing it is expensive because rebuilding it is slow.

### Why High-Trust Societies Work

High-trust societies (Nordics are the canonical example) don't work because people are inherently good. They work because of three structural properties:

1. **Trust by default.** You trust strangers because most people are trustworthy. This lowers transaction costs enormously — no lawyers for every handshake.
2. **Participation is valuable.** The system works well enough that being inside it is better than being outside it. Nobody is desperate enough that fraud is their best option.
3. **Asymmetric consequences.** Getting caught costs more than the fraud could gain. You're not just losing the stolen goods — you're losing access to a system that was already treating you well.

That third point is the key. The cost of fraud isn't punishment — it's exclusion from a network that was worth being in.

This transfers directly to federation:

| Nordic society | Allgard federation |
|---|---|
| Social safety net → nobody's desperate | Honest participation is profitable → fraud isn't the best strategy |
| Trust by default → low transaction costs | Greeter gives strangers access → low barrier to start |
| Getting caught → social and legal exclusion | Getting caught → reputation collapse, lost introductions, network exclusion |
| Reputation takes years to build | Trust levels take months to earn |
| Cost of fraud > benefit of fraud | Value of long-term network membership > value of one-shot extraction |

The goal isn't to make fraud impossible. It's to make the federation valuable enough that playing honestly is the dominant strategy for anyone with a time horizon longer than one transaction. Fraud becomes irrational, not just illegal.

**When this breaks down:** the same way it breaks in real societies — when someone has nothing to lose. A domain with no established reputation, no valuable relationships, no long-term interest in the network. That's why the bootstrap is slow and graduated. You can't extract much value before you've invested enough time that burning it would hurt.

That's the model. Everything below is the mechanical version.

## Introduction-Based Trust

Domains don't build trust by transacting with strangers. They build trust through introductions from domains they already trust.

### How It Works

1. **Domain A** trusts **Domain B** (established relationship, verified Proofs over time).
2. **Domain C** is new. C connects to B through the greeter, starts small transactions.
3. After C has a track record with B, **B introduces C to A** (Leden's `Introduce` operation).
4. A now knows: "B vouches for C." A starts interacting with C — cautiously, at lower limits than B gets.
5. Over time, A builds its own bilateral history with C. A's trust in C becomes independent of B.

### Introducer Accountability

This is the critical piece. When B introduces C to A, B is putting its own reputation on the line.

If C turns out to be fraudulent:
- A cuts off C (obvious)
- A downgrades B's introduction quality
- A may share this with other domains B has introduced (gossip)

B has every incentive to vet C before introducing C to anyone. A bad introduction costs B credibility with every domain that trusted B's judgment.

#### Don't Judge Intent — Judge Behavior

You can't tell the difference between "B didn't know C was a fraud" and "B was in on it." And you shouldn't try. Intent is unverifiable and any system that relies on it is gameable ("I had no idea!").

What you *can* judge is observable, verifiable behavior — before and after the fraud:

**Before the introduction:**

| Signal | What it tells you |
|--------|-------------------|
| B had sustained bilateral history with C | B had real data to base the introduction on — transaction timestamps are auditable |
| B introduced C after minimal interaction | B was careless or complicit — timestamps on B↔C history vs. introduction date don't lie |
| B introduced C at an appropriate trust level | B was calibrated, not overselling — the introduction carries a trust level |

**After the fraud was discovered:**

| Signal | What it tells you |
|--------|-------------------|
| B immediately flagged C to all its introductees | B is acting in good faith — gossip messages are timestamped |
| B cut off C | B isn't continuing to benefit — observable from transaction history |
| B stayed quiet until confronted | B was hoping nobody would notice — absence of gossip is observable |
| B continued trading with C after fraud was public | B is likely complicit — transaction history continues |

Every signal in these tables is externally verifiable. Self-reported claims ("I checked their Proofs thoroughly") are worth nothing — B can fabricate internal records after the fact. Only signals that other domains can independently confirm count.

This gives A enough information to calibrate its response to B without ever needing to determine whether B "knew." A domain that had years of history with C and responded immediately looks very different from one that introduced C after two weeks and went silent.

#### Penalty Scales with Reputation

A high-reputation domain that makes a bad introduction should take a bigger hit than a low-reputation one. Three reasons:

1. **Greater influence.** A's decision to trust C was partly based on B's reputation. The more weight B's introduction carried, the more responsible B is for the outcome.
2. **Greater obligation.** A trusted domain has more history, more access to audit data. It had more opportunity to detect problems.
3. **Greater signal.** When a highly-trusted domain makes a bad introduction, it's a stronger signal — either something is seriously wrong, or the domain is declining.

But penalties that scale too steeply create a problem: **established domains stop introducing anyone.** The downside of a bad introduction is catastrophic, so the rational move is to never introduce. The network ossifies. New domains can't get in. The high-trust society becomes a closed club.

The fix is to scale penalty with both reputation *and* observable behavior:

| Behavior (externally verifiable) | High reputation introducer | Low reputation introducer |
|---|---|---|
| Long B↔C history, fast fraud response, appropriate trust level | Small hit — honest mistake, could happen to anyone | Minimal hit — they did what they could |
| Short B↔C history, oversold trust level | Large hit — should have known better | Moderate hit — careless |
| Continued trading with C after fraud, silent on gossip | Severe — likely complicit | Large hit — enabling |

This means: a trusted domain that had years of history with an introduction and responded immediately to fraud takes a small, recoverable hit. A trusted domain that rubber-stamped a two-week relationship and went silent gets hammered. The incentive is to *take your time*, not to *never introduce*.

#### What Domains Track

| Metric | What it measures |
|--------|-----------------|
| Introduction success rate | % of B's introductions that resulted in valid long-term relationships |
| Introduction failure rate | % of B's introductions that resulted in fraud or cut-off |
| Introduction volume | How many introductions B has made (high volume + low failure = strong signal) |
| Average B↔introduced history length | How long B typically knows a domain before introducing it |
| Response time to fraud | How quickly B flagged problems after fraud was detected in its introductions |

All externally verifiable from transaction records and gossip timestamps. Nothing self-reported.

A domain with a 95% success rate over 500 introductions is a reliable introducer. A domain with 3 introductions total tells you nothing. A domain whose last 5 introductions were all fraudulent is actively dangerous. A domain with a 90% success rate but fast response times and long pre-introduction histories is honest but operating in a tough neighborhood.

### Why Introduce Anyone?

The accountability model explains what happens when introductions go wrong. But without a reason to introduce in the first place, the rational move is to never do it — all risk, no reward. The network dies in its crib.

Real trust networks don't have this problem because introducing people is *valuable*. The same dynamics apply here:

**Being a good introducer is a competitive advantage.**

A domain known for quality introductions becomes a hub. Other domains seek it out — not just for introductions, but because hubs attract activity. More connections = more trading partners = more liquidity = more opportunity. This is self-reinforcing: the better your introduction track record, the more domains want to connect through you, the more valuable your position becomes.

| Incentive | Mechanism |
|-----------|-----------|
| **Network growth** | A bigger network with more trusted participants benefits everyone. More trading partners, more asset types, more liquidity. Introducing good domains makes *your* network richer. |
| **Reciprocity** | I introduce your newcomers, you introduce mine. Bilateral introduction agreements reduce the cost of vetting for both sides. |
| **Introduction fees** | A domain can charge for introductions. Not protocol-mandated, but a natural business model. "I'll vouch for you to my contacts. Here's what that costs." Legitimate because the introducer's reputation is on the line — the fee reflects real risk. |
| **Hub status** | The domain with the best introduction track record becomes the first stop for newcomers. That's traffic, visibility, and influence. The equivalent of being the well-connected person everyone calls first. |
| **Early relationship advantage** | The domain that introduces C to the network gets the first bilateral relationship with C. If C turns out to be valuable, the introducer benefits from having been there first. |

**The economics work because introduction quality is visible.** A domain's introduction success rate, volume, and due diligence depth are all trackable (see [What Domains Track](#what-domains-track)). This means the market can price introduction quality. Domains with a 98% success rate over 1000 introductions can charge more — and domains seeking introduction will pay more — because the quality is demonstrated, not claimed.

**Compare to the real world:** headhunters, business brokers, venture capital intro networks. These exist because being the trusted connector is genuinely valuable. Nobody forces VCs to introduce startups to each other — they do it because their network *is* their product. The same dynamic emerges naturally in a federation where introduction quality is transparent and accountable.

### Network Health Incentives

Beyond introductions, domains have structural incentives to keep the network healthy:

**Participation in audit gossip is self-serving.** A domain that contributes audit evidence gets audit evidence back. If you help detect fraud against others, others help detect fraud against you. Free-riding (consuming gossip without contributing) is observable — and domains can deprioritize gossip to non-contributors.

**Flagging bad actors protects your own relationships.** When B discovers that C is fraudulent, B's fastest path to limiting damage is to warn every domain it introduced C to. Silence hurts B more than disclosure — B's contacts will discover the fraud eventually, and B's failure to warn becomes evidence of negligence (see [Don't Judge Intent — Judge Behavior](#dont-judge-intent--judge-behavior)).

**Reputation compounds.** The longer a domain maintains a clean track record, the more valuable its position. A domain with 5 years of clean history, 500 successful introductions, and fast fraud response times has something no money can buy and no shortcut can replicate. That accumulated reputation is the most valuable asset a domain holds — more valuable than any individual trade.

### Why This Resists Sybils

A Sybil attack under this model:

1. Attacker creates 10,000 domains
2. They all transact with each other — valid Proofs, conservation laws satisfied
3. They try to get introduced to legitimate domains

The bottleneck is step 3. Who introduces the Sybil cluster to the real network? The introducer needs an existing trusted relationship with legitimate domains. If the introducer is also a Sybil, it has no trusted relationships. If the introducer is a compromised legitimate domain, it burns its own reputation when the Sybils are detected.

The Sybil cluster can trade among itself forever. It can't extract value from the real network without a trusted introduction. And the introduction is traceable and accountable.

**Comparison to the real world:** A con artist can set up 50 shell companies that all do business with each other. Impressive on paper. But when they approach a real bank for a loan, the bank asks: "Who referred you?" If nobody credible referred them, they start at the bottom. If someone credible did refer them and they default, the referrer's credibility takes a hit too.

### Bootstrap Problem

How does the very first trust relationship form if everything requires introduction?

Two paths, depending on context.

**Path 1: Founding cluster (launch).** The first domains are run by people who already trust each other. They start at Allied trust level with pre-negotiated bilateral agreements, standard asset types, and a shared seed currency. No cold-start problem — trust is pre-established by convention, then formalized through the protocol. See [Founding Cluster](../allgard/README.md#founding-cluster).

**Path 2: New domain joining an established network.** You start at the bottom and work up slowly.

1. New domain connects to seeds, gets greeter capabilities
2. Greeter allows small transactions — below a threshold that limits damage
3. Over weeks/months of small, verified transactions, bilateral trust grows
4. Eventually the seed domain (or another established domain) is willing to introduce the new domain to others

Path 2 is deliberately slow. Building trust should be slow. That's the Sybil defense — there's no shortcut to a reputation that took six months to build. The founding cluster doesn't bypass this for outsiders — it only pre-establishes trust among its own members.

### Trust Levels

Domains set their own thresholds, but the model supports graduated trust:

| Level | Typical meaning | Typical requirements |
|-------|----------------|---------------------|
| Stranger | Greeter-only. Observe and small transfers. | None — any domain starts here |
| Known | Regular transactions, standard rate limits | Sustained bilateral history, no violations |
| Trusted | Higher limits, introduction-worthy | Long bilateral history, vouched by other trusted domains |
| Allied | Full bilateral agreement, pre-negotiated asset mapping | Formal agreement, mutual audit access |

These aren't protocol-level categories. They're a pattern that domains implement based on their own policies. A cautious domain might require a year of history for "trusted." A permissive domain might require a week. Sovereignty means each domain sets its own bar.

## Audit Gossip with Proofs

Current problem: if audit gossip is just claims ("I've verified 500 units from A"), it can be poisoned by Sybils injecting false reports.

**Rule: audit gossip carries evidence, not assertions.**

When Domain B tells Domain C about its observations of Domain A, the gossip message includes:

1. The specific Transforms B witnessed (hashes, not full content)
2. The Proofs B verified (or their hashes)
3. B's computed totals from those Proofs

C doesn't trust B's summary. C checks that B's totals are consistent with the Proofs B cites. C can also spot-check by fetching individual Proofs from B.

This means:
- **Fabricated gossip is detectable.** You can't claim "I verified 500 units" without producing 500 Proof hashes. And those hashes can be spot-checked.
- **Sybil gossip is bounded.** A Sybil can only gossip about transactions it actually participated in. Since Sybils can only transact with each other (no trusted introductions to real domains), their gossip about legitimate domains is empty.
- **Gossip is composable.** C can combine B's evidence with D's evidence and E's evidence to build a richer view of A's economy than any single domain has.

### Gossip Cost

Carrying Proofs makes gossip heavier than carrying summaries. Two mitigations:

1. **Hash-first, fetch-on-demand.** Gossip carries Proof hashes and totals. Full Proofs are fetched only when a domain wants to verify a specific claim. Most of the time, the hashes are enough — you're checking consistency, not re-verifying every Proof.

2. **Periodic, not continuous.** Supply audits don't happen on every transaction. Domains publish periodic audit snapshots (hourly, daily — domain policy). Trading partners verify at their own pace. The audit window means small transient discrepancies are tolerated; sustained fraud over audit periods is caught.

This addresses the "might get expensive" concern. You're not verifying every mint in real-time. You're periodically cross-checking snapshots, with the ability to drill into specific Proofs when something looks wrong.

## Slow-Burn Inflation Defense

The attack: a domain inflates supply by 1% per period, small enough that no single trading partner notices.

The defense is layered:

1. **Verifiable minting scripts (existing).** Every mint is a Raido script. The script's expected output is deterministic. If a domain claims to have minted 100 units and the script only produces 99, the discrepancy is mechanical.

2. **Cumulative bilateral views.** Domain B tracks every object it has ever received from A. Over time, B's accumulated view grows. A domain doing 1% inflation will eventually have exported more than its minting scripts can account for. The longer the fraud continues, the more trading partners accumulate evidence, and the harder it is to hide.

3. **Audit gossip aggregation.** When B, C, and D pool their evidence (with Proofs, not claims), the combined view exposes discrepancies that no single domain could detect. B saw 500, C saw 600, D saw 300 — total witnessed: 1400. If A's minting scripts only produce 1350, the 50-unit gap is visible.

4. **Mint log access (bilateral capability).** A domain seeking trusted/allied status offers its trading partners a `mint-log` capability: read access to the complete mint and burn history with Proof chains. Not every domain needs this — strangers and known domains operate on bilateral observations alone. But domains seeking higher trust offer deeper auditability.

This is graduated, not all-or-nothing. Small domains trading small amounts don't need full audit access. Large domains moving significant value negotiate deeper verification as part of their bilateral agreement.

## Collusion

Two trusted domains collude to fabricate assets. Both produce valid-looking Proofs for each other.

**I'm not trying to prevent this.** Two parties that collude can defraud each other. That's true in every system — two banks can collude to create fraudulent transfers between themselves, and no banking protocol prevents it.

What matters: **can they defraud a third party?**

No, if the third party does its own verification:

1. C receives an object from A. C verifies the minting Proof — re-executes the Raido script, checks the output. The script is content-addressed; A can't serve C a different script than A's legitimate one without changing the hash.

2. The colluded object was minted by a *legitimate* script with *fabricated inputs*. C re-executes with those inputs and gets the same output (determinism). But C can also check: do the inputs make sense? Does A's total supply, as computed from all minting scripts C can access, reconcile with A's self-reported numbers?

3. If A and B are colluding and only trading fabricated assets between themselves, the fraud is contained to their relationship. The moment they try to export fabricated value to C, C's bilateral view catches it — A is exporting more than its auditable minting can account for.

**Bounded damage.** Collusion between N domains damages those N domains and whoever trusts them blindly. Domains that verify independently are unaffected. The introduction model means: if A and B collude and then A introduces a Sybil to C, C traces the fraud back through A's introduction, and both A and B's introduction scores take a hit.

This is a known property, not a bug. The spec doesn't promise protection against N-party collusion. It promises that honest domains doing their own verification aren't harmed.

## Greeter Resource Limits

The greeter is the public entry point. Without limits, it's a DoS target.

| Limit | Purpose |
|-------|---------|
| Max concurrent stranger sessions | Prevent connection exhaustion |
| Connection rate per source | Prevent rapid reconnection after disconnect |
| Observation bandwidth cap per stranger | Prevent catalog/gossip flooding |
| Transfer size cap for strangers | Bound exposure to unknown domains |
| Transfer rate cap for strangers | Bound transaction volume from unknowns |

These are domain-configurable but must be present. A domain with no greeter limits is vulnerable by choice. The defaults should be conservative — tighten later if needed, but start restrictive.

Leden's session-level backpressure handles the transport side. These are Allgard-level limits on what a stranger can *do*, not how fast bytes flow.

## Reputation Laundering

The attack: Domain A builds reputation, commits fraud, shuts down. Spins up A' with new keys. Clean slate.

The defense is the cost of rebuilding. Under this trust model:

- A' starts as a stranger. Greeter-only, small transactions.
- A' has no introductions. Nobody vouches for it.
- Building back to "trusted" takes the same months/years it took the first time.
- The fraud A committed lives in every domain that witnessed it. If A' is run by the same operator (same IP ranges, same behavior patterns, same trading partners), domains can flag it. This isn't protocol-level — it's domain-level heuristics. But the information is available.

**The honest cost:** a determined attacker with patience can launder reputation. The question is whether the payoff exceeds the cost of months of legitimate-looking operation. For most attacks, it doesn't. For nation-state level attackers — nothing in any protocol stops them.

The goal isn't perfect prevention. It's making the cost high enough to deter most fraud and containing the damage when it happens.

## Eclipse Attacks

Isolating a domain from gossip so it can't learn about a fraudulent domain's exposure.

This is a Leden concern, not an Allgard concern. But Allgard's trust model has a natural mitigation: a domain with diverse introduction sources (trust relationships through multiple independent introducers) gets gossip from multiple independent paths. Eclipsing requires controlling all of them.

**Recommendation:** domains should maintain trust relationships with multiple independent clusters. If all your trusted contacts were introduced by the same domain, you have a single point of failure. Diverse introduction sources = diverse gossip sources = eclipse resistance.

## Attack Summary

| Attack | Defense | Strength |
|--------|---------|----------|
| Sybil reputation mining | Introduction-based trust; introducer accountability | Strong — bottleneck is getting trusted introductions |
| Audit gossip poisoning | Gossip carries Proof hashes, not claims; spot-checkable | Strong — fabricated evidence is detectable |
| Slow-burn inflation | Cumulative bilateral views + periodic audit snapshots + mint-log capability | Strong over time — harder to hide as evidence accumulates |
| Collusion (N-party) | Contained to colluding parties; independent verifiers unaffected | Bounded — honest domains aren't harmed |
| Reputation laundering | Slow trust building; no shortcut to reputation | Moderate — patient attackers can do it |
| Eclipse | Diverse trust relationships; multiple gossip paths | Moderate — network-level defense, not protocol-level |
| Greeter DoS | Connection/rate/bandwidth limits | Operational — configurable per domain |

## Transparency Is the Security Model

Domains aren't people. They're services. A domain that won't tell you who it trades with is like a business that refuses to name its suppliers. That's not privacy — that's a red flag.

Introduction graphs should be visible. Audit gossip should be public. Trading relationships should be inspectable. The honest domain has nothing to hide. The one that whispers is the one you should worry about. This is old wisdom and it applies directly: the transparency *is* what makes the trust model work. Making introduction graphs private gives Sybil clusters a place to hide.

### Prove Properties, Not Data

That said, there are cases where you want to prove something about your history without dumping every record. A domain should be able to say "I have 50 trusted introductions with a 97% success rate" and back that claim cryptographically without listing all 50 domains.

Commitment schemes work here. A domain publishes a Merkle root over its introduction set. A verifier can check:
- The set has N members (tree size)
- A specific domain is or isn't in the set (inclusion/exclusion proof)
- Aggregate statistics are consistent with the committed data

This isn't privacy for its own sake. It's efficiency — you don't always need the full dataset to make a trust decision. Sometimes the summary, backed by a commitment you can drill into if something looks wrong, is enough.

**The principle:** default to transparent. Use commitments for efficiency, not secrecy. Any domain can request full disclosure as a condition of higher trust levels. Refusing disclosure is itself a signal.

## Stress Test

I tried to break this model. Here's what I found.

### Greeter Extraction Loop (not a real attack)

**Attack:** Create domain, extract value at stranger level, burn identity, repeat.

**Why it doesn't work:** What value? A stranger connects to a greeter. The greeter gives observation capabilities and a transfer inbox. The stranger can *offer* things to the domain. The domain decides whether to accept. A domain has no reason to hand value to a stranger for free.

The transfer inbox lets a stranger submit transfers — but [Conservation Law 3](CONSERVATION.md) requires value in = value out. A stranger can't receive more than they give. If they have nothing to give (brand-new domain, nothing minted), they can only observe.

This is a non-attack. The greeter is already safe by construction — the domain controls what it gives, and the conservation laws prevent something-for-nothing.

### Patient Sybil Coordination (accepted cost — every network has this)

**Attack:** 100 domains, each builds reputation over 6 months with legitimate behavior. Coordinated attack on day 181.

**How every network handles this:** They don't prevent it. They budget for it.

Banks lose billions annually to fraud. Credit card companies build fraud losses into interchange fees. Insurance companies have actuarial tables. Every trust network in human history has accepted that some fraction of participants will defect. The goal isn't zero fraud — it's keeping expected losses below the value the network creates.

What this trust model does:
- **Makes the attack expensive.** 6 months × 100 domains of real infrastructure and real trading is a real cost.
- **Bounds the damage per domain.** Trust levels limit how much any single domain can extract. A domain with 6 months of history doesn't get unlimited credit — it gets limits proportional to its demonstrated history.
- **Makes coordinated defection detectable.** Transparency means 100 domains going dark is visible. The network can respond — clawbacks (see [Dying Domain Endgame](#dying-domain-endgame)), introduction chain analysis, reputation downgrades for introducers.
- **Makes recovery automatic.** Cut off the defectors, downgrade their introducers, continue. The network doesn't need to "fix" anything — the defectors burned their own positions.

The remaining question is whether the expected payout exceeds the cost. The trust model's job is to push that ratio as far toward "not worth it" as possible. It can't push it to zero. No system can.

### Introduction Laundering via Cutouts (detectable — graph is transparent)

**Attack:** B introduces legitimate C. B's sock puppet D befriends C independently. C introduces D. D defrauds everyone. C takes the hit, B is untouched.

I initially called this "hard to fix." It's not — because the introduction graph is transparent.

**Every introduction is a public record.** The full chain is visible: B introduced C, C introduced D, D defrauded everyone. Any domain can compute: "what fraction of downstream fraud traces back through B's introduction chains?" One incident is noise. A pattern is signal.

**Downstream fraud score.** Domains can track not just direct introduction failures, but introduction failures N hops downstream. B's direct introductions might all look clean — but if B's introductions consistently lead to fraud two hops later, that's a measurable anomaly. The transparent graph makes this computable.

The score naturally attenuates with distance — one hop is strong signal, two hops is weaker, three hops is noise. But the cutout pattern specifically creates a two-hop signature that's detectable over repeated attacks.

**Single cutout attacks are still undetectable.** One instance of B→C→D where D defrauds doesn't implicate B. That's fine — one instance of anything is indistinguishable from bad luck. The trust model handles fraud as a statistical property, not a per-incident investigation.

### Hub Centralization (structural limit — Law 7)

The incentive model pushes toward hub formation. Without a structural constraint, power laws apply and a few hubs dominate.

I don't want to just document this risk and hope for the best. Federation's track record with "the market will sort it out" is poor. Gmail dominates email. Centralized platforms ate XMPP.

**Structural fix: bounded introduction rate.**

A domain can introduce at most N new domains per time period. This is the same principle as Law 5 (bounded rates) applied to introductions. Call it a natural limit on how fast any single domain can vouch for newcomers.

Why this works:
- **Caps hub dominance.** A domain that can only introduce 10 new domains per month can't become the sole gateway to the network. Others must share the load.
- **Forces distributed introduction.** Newcomers can't all funnel through one hub. They must find multiple introduction paths, which naturally diversifies the graph.
- **Quality over quantity.** A limited introduction budget means each introduction is more valuable and worth more due diligence. You don't waste your 10 monthly introductions on domains you haven't vetted.
- **Scales with trust level.** The introduction rate limit can scale with the introducer's own trust level — allied domains get a higher budget than known domains. This is earned, not assumed.

**What this doesn't solve:** A hub can still become *influential* — being the most trusted introducer carries weight even if capped. But it can't become a *monopoly*. There's a structural ceiling on introduction concentration.

**Enforcement:** Same as Law 5. The introduction carries a timestamp. Domains that receive more than N introductions from the same source in a time window can reject the excess. No central enforcer needed — it's bilateral verification of a rate limit.

### Gossip Is a Duty (structural requirement)

I initially framed audit gossip participation as an incentive ("self-serving"). That's too weak. Conservation Laws are duties, not suggestions. Gossip should be too.

**Requirement: every domain that participates in cross-domain transfers must contribute to audit gossip.**

What "contribute" means:
- Share bilateral observations (with Proof hashes) when requested by trading partners
- Propagate fraud reports (with evidence) received from other domains
- Respond to supply audit queries for asset types the domain mints

**Non-participation is observable and consequential.** A domain that consumes gossip without contributing is detectable — its trading partners ask for observations and get silence. Consequences:

- Trading partners can downgrade the non-contributing domain's trust level
- Introduction quality scores can factor in gossip participation
- At the extreme: domains can refuse to trade with non-contributors

This isn't a new conservation law — it's an enforcement mechanism for the existing ones. The conservation laws are only as strong as the bilateral verification that checks them. Gossip is how that verification scales beyond direct trading partners. Without it, fraud detection is limited to direct bilateral views, which is weaker.

**Minimum viable gossip:** A domain doesn't need to gossip with everyone. It must gossip with its direct trading partners. Those partners gossip with their partners. Information propagates through the network's existing trust graph. The duty is local — gossip with the domains you trade with. The effect is global.

#### Gossip as Runtime

The duty framing is correct at the model level. But at the implementation level, gossip is not an operational task — it's a runtime concern.

A domain operator installs the software, configures game logic, runs their world. The federation infrastructure — gossip participation, audit verification, reputation tracking, fraud report propagation — runs automatically as part of the Leden/Allgard runtime. Zero-config for the base case.

The analogy is TCP congestion control. It's a protocol duty with real consequences (violators get poor throughput). But nobody "operates" congestion control. It's built into the stack. Same principle here.

What this means concretely:
- **Gossip participation** is a default-on runtime behavior, not a config option an operator enables
- **Audit verification** runs automatically when trading partners request it
- **Reputation tracking** accumulates in the background from observed transactions and gossip
- **Fraud report propagation** happens automatically when evidence arrives

Operators can tune parameters — audit frequency, gossip fanout, reputation thresholds. They can't turn off participation without turning off federation. The runtime participates because participating is what makes the conservation laws enforceable beyond direct bilateral views.

This is part of a broader principle: [federation should be invisible](../allgard/README.md#invisible-federation). The operator's job is domain logic. The federation infrastructure handles itself.

### Reputation As a Weapon (all gossip requires proof)

**Attack:** Domain A claims "C was fraudulent" to damage C's reputation.

**Rule: all gossip carries evidence.** This was already decided for audit gossip — extend it to everything.

When A claims C defrauded it, A produces:
- The specific Transforms that constitute fraud
- The Proofs that show the violation
- Which Conservation Law was broken

"C was fraudulent" without evidence is an opinion. Opinions don't propagate through gossip — they stay between A and whoever A talks to directly. Only evidence-backed fraud reports propagate.

**Distinction the protocol enforces:**

| Report type | Propagates? | Requires |
|------------|-------------|----------|
| Fraud report | Yes — gossip carries it | Proof of Conservation Law violation |
| Relationship ended | Visible (no transactions) but not propagated as gossip | Nothing — it's just observable absence |
| Subjective distrust | No | Nothing — it's A's private policy |

A domain that frequently ends relationships without fraud evidence looks erratic, not authoritative. Weaponized reputation attacks are bounded: you can cut off whoever you want, but you can't damage their network reputation without proof.

### Dying Domain Endgame (clawback mechanism)

A domain going bankrupt drains everything it can. Legitimate domain turned rogue.

**Existing mechanics bound the damage:**

1. **Single ownership (Law 2) + authority scoping (Law 6):** The domain hosts objects but doesn't own them. The operator can't transfer objects without the Owner's key signing the Transform. A rogue operator can't forge player signatures.

2. **What the operator CAN do:** refuse to process outbound transfers (denial of service), or manipulate domain-owned assets (treasury, minted currency). The first is bounded by players moving to other domains. The second is bounded by what the domain legitimately owns.

3. **Detection is fast.** A domain that stops processing transfers or starts irregular minting triggers audit gossip from every trading partner simultaneously.

**New mechanism: clawback window.**

Cross-domain transfers from a domain that goes dark within N days of a transfer can be flagged for review by the receiving domain. This is the same principle as corporate bankruptcy clawback — transfers made shortly before insolvency are presumed suspicious.

How it works:
- Every cross-domain transfer carries a timestamp and causal link (Law 4 — already exists)
- If Domain A goes dark (stops responding, stops participating in gossip, stops processing transfers), its trading partners mark the timestamp
- Transfers received from A within the clawback window (configurable per bilateral agreement — e.g. 30 days) are flagged
- The receiving domain can choose to: hold the assets in escrow pending investigation, reverse the transfer (return assets to their original owners if contactable), or accept the transfer (if the domain judges it legitimate)

**What this catches:** A dying domain that dumps its treasury to accomplice domains in the days before going dark. The accomplice domains receive the assets but they're flagged. Other trading partners can see (through gossip) that A went dark and recently transferred large values. The accomplice's willingness to accept flagged assets is itself a signal.

**What this doesn't catch:** Slow extraction over months that looks like normal trading. But that's the patient Sybil problem — bounded by trust levels and cumulative bilateral views.

**Key principle:** The domain is a host, not an owner. Player-owned objects must use player-controlled keys, never domain-controlled keys. A domain that controls its players' keys has the power to steal from them. The protocol should make this an explicit recommendation — domain operators who hold player keys are undermining the ownership model.

## Open Questions

- **Clawback window duration.** What's the right default? Too short and dying domains can extract before it. Too long and legitimate domain shutdowns (planned migrations) get flagged unnecessarily. Probably needs to be configurable per bilateral agreement, with a protocol-suggested default.
- **Downstream fraud score attenuation.** How quickly does introduction accountability decay over hops? One hop is clear. Two hops (cutout detection) is useful. Three hops is probably noise. The exact attenuation curve needs thought — too steep and cutouts work, too shallow and the whole network is punished for being connected.
