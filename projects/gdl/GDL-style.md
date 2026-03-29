Gard Description Style
<!-- id: gdl.style --> <!-- status: proposed --> <!-- summary: Visual identity and theming system for GDL — design tokens, structured hints, and CSS stylesheets -->

GDL-style is GDL's CSS. GDL describes what exists — structure, entities, affordances. GDL-style describes how it should feel — colors, atmosphere, lighting, sound palette, typography, entity treatment. Separate specs because they evolve independently and have different implementer audiences.

Without GDL-style, GDL is the pre-CSS internet. Content exists. Visual identity doesn't. A horror dungeon and a fairy forest render with the same client defaults. Walking through a cross-domain portal feels like nothing because both sides look identical. Domain authors have no way to express creative intent beyond text descriptions and per-entity appearance layers.

CSS solved this for documents. GDL-style solves it for worlds. But GDL-style is not CSS — CSS styles text, boxes, and layout. GDL-style styles lighting, atmosphere, color language, entity treatment, and sound. Different medium, different tool.
Why Not Just CSS

I considered making the entire style system CSS with custom properties and GDL-specific selectors. Three problems:

1. CSS has no concept of fog density, ambient light color, or shadow intensity. You'd need custom properties for all of them, which makes CSS a transport format for non-CSS data. At that point it's not CSS anymore — it's key-value pairs wearing CSS syntax.

2. CSS selectors are overkill. GDL-style doesn't need `.creature[data-hostile="true"]:nth-child(2n+1)`. It needs "hostile creatures get a red tint." Flat token namespaces (`entity.hostile_tint: #ff2200`) are simpler to author, simpler to parse, and impossible to create specificity bugs with.

3. CSS is still needed — for panels and web client UI. If the whole style system were CSS, you'd mix world-styling custom properties with actual layout CSS in one file. Keeping them separate means domain authors know exactly what goes where: tokens for world feel, CSS for panel/UI appearance.

The architecture: tokens + hints (GDL-style, client-agnostic) and stylesheets (CSS, for web clients). Tokens are the real design system. CSS is a bonus layer.
Design Tokens

Tokens are named values that express visual identity. They're the bridge between "what the domain wants" and "what the client renders." Every client type can consume tokens — a text client maps color tokens to ANSI terminal colors, a 2D client maps them to sprite tinting, a 3D client maps them to shader uniforms.

Tokens are flat key-value pairs with dotted namespaces. No selector syntax. No cascade complexity. A domain author can write them in five minutes.

theme:
  tokens:
    # Color language
    color.primary: #2a1a0e
    color.secondary: #5c3a1e
    color.accent: #8b6914
    color.surface: #1a1a1a
    color.background: #0d0d0d
    color.text: #c4a882
    color.danger: #8b2500
    color.success: #2e5a1e
    color.warning: #8b6914

    # Atmosphere
    atmosphere.fog_color: #1a1008
    atmosphere.fog_density: 0.4
    atmosphere.ambient_color: #3a2a1a
    atmosphere.ambient_intensity: 0.3
    atmosphere.shadow_intensity: 0.8
    atmosphere.sky_color: #0a0a0a
    atmosphere.sky_intensity: 0.1

    # Entity treatment
    entity.hostile_tint: #ff2200
    entity.friendly_tint: #44aa44
    entity.interactive_highlight: #ffcc00
    entity.selected_outline: #ffffff
    entity.damaged_overlay: #880000

    # Kind defaults — override per-entity appearance
    kind.creature.label_color: #c4a882
    kind.item.highlight: #8b6914
    kind.portal.glow_color: #4488ff
    kind.portal.glow_intensity: 0.6
    kind.structure.label_visible: false
    kind.terrain.label_visible: false

    # Typography
    type.heading: serif
    type.body: serif
    type.ui: sans-serif

    # Sound palette
    sound.ambient_layer: sha256:aa11bb...
    sound.music_mood: dark_ambient
    sound.interaction: stone_click
    sound.footstep: wood_creak

  # Structured hints (see below)
  mood: gritty
  epoch: medieval
  saturation: low
  contrast: high
  density: cluttered

  # CSS stylesheet for panels and web clients
  stylesheet: sha256:ef9a01...
Token Categories

Category	Namespace	Purpose
Color language	color.*	Domain's palette — primary, secondary, accent, surface, danger, etc.
Atmosphere	atmosphere.*	Lighting, fog, sky, shadows — the environmental feel
Entity treatment	entity.*	Visual treatment of entity states — hostile, selected, damaged, interactive
Kind defaults	kind.<kind>.*	Default appearance per entity kind — label visibility, glow, highlight
Typography	type.*	Font family hints by role — heading, body, UI
Sound palette	sound.*	Ambient layers, music mood, interaction sounds, footstep style

How each client type maps each category:

Color:
- Text client → ANSI terminal colors (color.danger → red, color.success → green)
- 2D client → sprite tinting, UI palette, overlay colors
- 3D client → material uniforms, UI palette, post-processing color grading
- Web client → CSS custom properties (--gdl-color-primary, etc.)

Atmosphere:
- Text client → ignored (mood comes from description text)
- 2D client → screen-space fog overlay, global brightness, sky color
- 3D client → fog shader, skybox, shadow maps, ambient light, volumetrics
- Web client → CSS filters on game canvas, overlay layers

Entity treatment:
- Text client → prefix markers ([!] for hostile, [*] for interactive)
- 2D client → outline colors, tint overlays, highlight animations
- 3D client → outline shaders, fresnel effects, particle highlights
- Web client → CSS classes toggled on entity DOM elements

Kind defaults:
- All clients → fallback appearance when entity has no explicit iconic/scene layers
- Overridable per-entity by the entity's own appearance field
- kind.structure.label_visible: false means structure names hidden by default (reduce clutter)

Typography:
- Text client → ignored (terminal font)
- 2D/3D client → font selection for entity labels, UI text
- Web client → CSS font-family on UI elements

Sound:
- Text client → ignored
- Audio-capable client → ambient audio layer, music selection, interaction sound effects
- sound.music_mood is a vocabulary term; the client maps it to its music library
Token Value Types

Type	Example	Notes
color	#2a1a0e, #ff2200	Hex RGB. Alpha via #RRGGBBAA. Clients map to their color space.
float	0.4, 0.8	0.0–1.0 for intensities/densities. Unbounded for scale/size.
bool	true, false	Visibility flags, toggles.
string	serif, dark_ambient	Vocabulary terms. Clients map to their asset/font libraries.
ref	sha256:aa11bb...	Content-addressed blob reference (for sound layers, textures).
Cascade

Tokens cascade from domain → region → entity. Three levels, strict precedence, no specificity.

A domain declares its global identity at session start (returned by the greeter alongside the initial region). Every region inherits domain tokens. A region's theme overrides specific tokens. An entity's appearance overrides everything for that entity.

domain_theme:
  tokens:
    color.primary: #2a1a0e
    color.accent: #8b6914
    atmosphere.fog_density: 0.2
    # ... domain-wide identity

region_theme:
  tokens:
    atmosphere.fog_density: 0.8    # this dungeon is foggier
    atmosphere.ambient_intensity: 0.1  # and darker
    # color.primary: inherited from domain

entity appearance:
  palette: [#ff0000]  # this specific entity overrides its kind default

The domain level is the brand. The region level is the scene. The entity level is the individual. Last writer wins. Entity beats region. Region beats domain. No complex specificity rules.

Region-level atmosphere scripts (see [GDL-extensions: Client Scripts](GDL-extensions.md#client-scripts)) add a computed layer: the script outputs token overrides derived from region properties (time_of_day, weather). Script outputs override static region theme tokens of the same name but are overridden by entity appearance. The full precedence: domain theme → region theme → region script outputs → entity appearance. Scripts only affect tokens they explicitly produce; other tokens cascade normally.

A domain with 50 regions doesn't repeat its color palette 50 times. It defines it once. Dark dungeons override atmosphere tokens. The bright overworld overrides them differently. Individual entities stand out when they need to.

Token inheritance is merge, not replace. A region that sets `atmosphere.fog_density: 0.8` inherits all other domain tokens unchanged. Only the explicitly overridden tokens change.
Structured Hints

Alongside tokens, themes carry structured mood hints. These are coarser than tokens — high-level signals for clients that don't want to interpret individual token values.

Field	Values	Purpose
mood	gritty, whimsical, serene, ominous, epic, mundane, alien, sacred, decayed, mechanical	Emotional register — the single most useful hint
epoch	medieval, futuristic, modern, ancient, alien, steampunk, mythic, ...	Time period — drives asset selection, font choice, sound design
saturation	low, medium, high	Color intensity guidance
contrast	low, medium, high	Lighting contrast guidance
density	sparse, normal, cluttered, dense	How full the space feels

These are shortcuts. A client that fully supports tokens can ignore structured hints — they're derivable from token values. A simpler client that doesn't parse tokens can use mood + epoch to pick a preset. "Gritty medieval" → dark, muted rendering. "Whimsical futuristic" → bright, neon. The structured hints are the minimum viable theme for clients that don't want the full token system.

Structured hints also cascade (domain → region), same as tokens.

Mood vocabulary:
Mood	Guidance
gritty	Dark, rough, dangerous. Muted colors, harsh lighting, worn textures.
whimsical	Light, playful, colorful. Rounded shapes, bright palette, bouncy feel.
serene	Calm, peaceful. Soft colors, gentle lighting, low contrast, slow ambient.
ominous	Threatening, foreboding. Deep shadows, desaturated, unsettling sound.
epic	Grand, dramatic. High contrast, saturated, sweeping scale, orchestral.
mundane	Ordinary, everyday. Natural colors, neutral lighting, realistic.
alien	Strange, unfamiliar. Non-standard palette, asymmetric, dissonant sound.
sacred	Reverent, holy. Gold accents, soft glow, echo, vertical emphasis.
decayed	Abandoned, rotting. Desaturated, broken geometry, organic overtones.
mechanical	Industrial, precise. Metal palette, rhythmic sound, grid alignment.
Stylesheets

The theme's stylesheet field is a content-addressed CSS blob. This is actual CSS — the same language web developers already know. It handles two things that tokens can't: panel layout and web client UI treatment.

Purpose 1: Panel styling. Panels are HTML fragments. Without a stylesheet, they render in the client's default style — a horror domain's skill tree gets cheerful blue buttons. The stylesheet gives panels visual identity: colors, fonts, spacing, borders that match the domain's mood.

Purpose 2: Client UI theming. Web-capable clients can adopt the stylesheet's CSS custom properties for their own UI chrome. When the player walks through a portal, health bars, menus, and overlays shift to match the new domain's palette.
Token-to-CSS Bridge

Clients that support CSS automatically expose tokens as custom properties:

    :root {
      /* Auto-generated from theme tokens */
      --gdl-color-primary: #2a1a0e;
      --gdl-color-secondary: #5c3a1e;
      --gdl-color-accent: #8b6914;
      --gdl-color-surface: #1a1a1a;
      --gdl-color-background: #0d0d0d;
      --gdl-color-text: #c4a882;
      --gdl-color-danger: #8b2500;
      --gdl-color-success: #2e5a1e;
      --gdl-type-heading: serif;
      --gdl-type-body: serif;
      --gdl-type-ui: sans-serif;
      /* ... all tokens mapped to --gdl-{namespace}-{name} */
    }

The mapping is mechanical: `color.primary` → `--gdl-color-primary`. `atmosphere.fog_density` → `--gdl-atmosphere-fog-density`. Dots become hyphens. The `--gdl-` prefix prevents collisions with the client's own custom properties.

The domain's stylesheet builds on these. It doesn't re-declare the palette — it references the tokens:

    .gdl-panel {
      background: var(--gdl-color-surface);
      color: var(--gdl-color-text);
      font-family: var(--gdl-type-body, serif);
      border: 1px solid var(--gdl-color-accent);
    }

    .gdl-health-bar {
      background: var(--gdl-color-danger);
      border-radius: 2px;
    }

    .gdl-affordance-button {
      background: var(--gdl-color-surface);
      color: var(--gdl-color-accent);
      border: 1px solid var(--gdl-color-accent);
    }

    .gdl-affordance-button:hover {
      background: var(--gdl-color-accent);
      color: var(--gdl-color-surface);
    }

If the domain changes its tokens (e.g., a day/night cycle shifts color.primary), the CSS custom properties update automatically and the stylesheet's var() references resolve to the new values. No stylesheet swap needed for token-driven changes.
Well-Known CSS Classes

Clients that render HTML-based UI should expose these classes on their elements. Domain stylesheets target them to style the client's built-in chrome.

Class	Element	Purpose
.gdl-panel	Panel container	Wraps each domain panel
.gdl-health-bar	Health indicator	Bar showing health/health_max ratio
.gdl-health-bar-fill	Health fill	Inner element sized to health/health_max
.gdl-entity-label	Name label	Entity name overlay in graphical clients
.gdl-entity-label--hostile	Hostile variant	Label on hostile entities (additional class)
.gdl-affordance-button	Action button	Clickable affordance trigger
.gdl-affordance-menu	Menu container	Groups affordances by category
.gdl-affordance-group	Category group	One affordance category within the menu
.gdl-region-name	Region title	Current region's name display
.gdl-ambient-overlay	Screen overlay	Full-screen mood/atmosphere tinting layer
.gdl-toast	Notification	Transient messages (damage numbers, pickups)

The client is not required to apply domain styles to its own UI. But clients that do get the portal-transition effect: walk into a new domain and the entire UI shifts color and feel. This is the key experience that makes cross-domain travel feel coherent rather than jarring.

Modifier classes follow BEM-lite convention: `.gdl-entity-label--hostile`, `.gdl-affordance-button--disabled`, `.gdl-panel--collapsed`. Domains can target these for state-specific styling.
Stylesheet Constraints

- No JavaScript. No `<script>`, no event handlers, no `expression()`, no `-moz-binding`.
- No external resources. No `@import`, no `url()` pointing to external hosts.
- Content references only. `url()` values must be `data:` URIs or `sha256:` content-addressed references (resolved through Leden's content store).
- No layout hijacking. Clients may reject or sandbox properties that change their layout structure: `position: fixed/absolute`, `z-index` above client-reserved range, `display: none` on critical UI, `pointer-events: none` on interactive elements. The stylesheet styles appearance, not structure.
- Size limit. 64KB maximum. Enough for comprehensive theming. Too small for embedded fonts or image data URIs (those go in the content store as assets).
- Scoping. Clients should scope domain stylesheets to prevent cross-domain style leaks. When the player has panels from two domains visible, each domain's stylesheet applies only to its own panels. CSS `@scope` or shadow DOM provides this.

Clients validate stylesheets before applying. Parse the CSS, strip disallowed properties, verify all url() references. A malicious domain cannot use a stylesheet to exfiltrate data, track users, overlay misleading UI, or escape sandboxing.
Theme Updates

Themes can change during a session. A day/night cycle shifts atmosphere tokens. An event tints the region red. A quest completion changes the mood from ominous to serene.

Theme changes arrive through the observation stream as a `theme_update` delta on the region:
Update	Payload	When
theme_update	Changed tokens and/or hints	Region visual identity changes

Token updates are partial — only changed tokens are sent. The client merges them with the current token set.

For dynamic environments (day/night cycles, weather), domains can use region-level atmosphere scripts (see [GDL-extensions: Client Scripts](GDL-extensions.md#client-scripts)) instead of streaming `theme_update` deltas. An atmosphere script takes `time_of_day` as input and computes `atmosphere.ambient_intensity`, `atmosphere.fog_density`, etc. locally on the client. This replaces hundreds of per-frame token updates with a single property change — the domain sends `time_of_day: 0.75` and the client derives the entire atmosphere. More efficient, zero-latency transitions, and the domain can verify the output because the script is deterministic.

`theme_update` deltas remain the mechanism for discrete token changes (quest completion shifts mood, portal arrival changes palette) and for clients that don't support scripts.

Stylesheet changes send a new content hash. The client fetches the new blob and re-applies. Stylesheet swaps are infrequent (portal transitions, major events), not per-frame.
How Clients Consume Themes — Full Example

The same theme applied by four client types:

Text client:
- Maps color.danger → ANSI red for hostile creature names
- Maps entity.hostile_tint → a [!] prefix on hostile entities
- Uses mood ("gritty") to adjust description framing: "you sense danger" vs. neutral listing
- Ignores atmosphere, typography, sound, stylesheet entirely
- Result: the horror dungeon FEELS different through word choice, even in text

2D tile client:
- Maps color.* to UI palette and sprite tinting
- Maps atmosphere.fog_density → screen-space fog overlay (dark gradient from edges)
- Maps atmosphere.ambient_intensity → global brightness multiplier
- Maps kind.portal.glow_color → portal tile animation tint
- Maps sound.ambient_layer → background audio loop
- Uses mood + epoch to select sprite set variant (if multiple available)
- Applies stylesheet to any HTML panels it renders
- Result: same tiles, but the color grading and atmosphere make it feel like a dungeon

3D client:
- Maps color.* to material tints and UI palette
- Maps atmosphere.* directly to fog shader, skybox color, shadow map intensity, ambient light
- Maps entity.* to outline/highlight shader parameters
- Maps kind.* to default material/glow when entities lack custom appearance
- Maps sound.* to spatial audio: ambient layer, reverb preset, footstep sound selection
- Applies stylesheet to HUD panels (typically HTML overlays in the 3D engine)
- Result: full atmospheric rendering — fog, lighting, color grading, spatial audio, all from tokens

Web client:
- Injects all tokens as CSS custom properties on :root
- Applies stylesheet directly to page
- Uses .gdl-* classes on its UI elements
- Gets portal-transition effect: region change → new tokens → CSS custom properties update → instant UI recolor via var() references
- Panels inherit styling automatically via the CSS cascade
- Result: the domain's visual identity permeates every pixel of the client
Resolved

Tokens over selectors. I considered CSS-like selectors for targeting entities (`creature[hostile=true] { tint: red }`). Flat namespaced tokens are simpler. No specificity bugs. No parsing complexity. Domain authors write `entity.hostile_tint: #ff2200` instead of learning a selector language. The tradeoff is less precision — you can't target "hostile creatures of level > 10 in this specific room." But that level of granularity belongs in per-entity appearance, not in the theme.

Separate spec from GDL. CSS is a separate spec from HTML. They evolved independently with different versioning, complexity budgets, and implementer communities. GDL-style changes more frequently than GDL — new token categories, new mood terms, stylesheet feature additions. Keeping them separate means GDL implementations are complete without GDL-style (use client defaults), and GDL-style can evolve without forcing GDL revisions.

Three-level cascade is enough. I considered adding a fourth level (union themes — a domain union defines shared visual standards). Not worth the complexity. If a union wants visual consistency, its member domains use the same domain-level tokens. The cascade doesn't need to know about unions.

Structured hints AND tokens, not OR. Some people will say "just use tokens" or "just use mood hints." Both camps are wrong. Tokens give precise control for capable clients. Structured hints give useful signals for simple clients that don't want to parse 40 tokens. A text client benefits from `mood: ominous` even though it ignores every token. A 3D client benefits from `atmosphere.fog_density: 0.4` even though `mood: gritty` is too vague for its renderer.

CSS for panels, tokens for worlds. The stylesheet handles document-like concerns (panel layout, fonts, borders). Tokens handle world-like concerns (fog, lighting, entity treatment). Mixing them in one system would mean CSS parsing is required for atmosphere rendering, which kills the "buildable in a weekend" promise for simple clients.
Open Questions

Token animation. Resolved: no transition hint syntax. Region-level atmosphere scripts (see [GDL-extensions: Client Scripts](GDL-extensions.md#client-scripts)) handle this natively. An atmosphere script takes `time_of_day` and computes smooth token values over time — the interpolation happens in Raido, not in the token format. For domains without scripts, clients interpolate `theme_update` deltas at their own rate. Adding transition hints to the token format would couple GDL-style to a specific interpolation model and duplicate what scripts already do.

Audio token depth. The sound.* namespace here stays shallow — ambient layer, music mood, footstep style. These are mood/identity tokens (what the domain sounds like), not acoustic environment parameters. Physical acoustics (room volume, absorption, occlusion, emission radius) are specified in [GDL-extensions: Acoustic Environment](GDL-extensions.md#acoustic-environment) as region and entity properties. The split: GDL-style tokens say "this place sounds dark and gritty." Acoustic properties say "this room is 450 cubic meters of stone with 0.1 absorption." Different concerns, different specs.

Accessibility overrides. The client's accessibility settings should always win over domain themes. A domain that sets `contrast: low` shouldn't override a user's high-contrast mode. The cascade should probably be: domain → region → entity → USER (always wins). But this means the client needs to know which tokens map to accessibility-relevant settings. Not specified yet.

Theme previews on portals. When the client is near a portal, should it receive the destination region's theme in advance? This would enable pre-loading assets and showing a visual preview of what's on the other side (color bleeding through the portal, for instance). Adds bandwidth but makes transitions smoother.
