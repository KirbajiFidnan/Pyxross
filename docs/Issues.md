# UX and Product Issues

This document records the current usability complaints and product gaps
reported during hands-on evaluation. These are tracked issues, not an
implementation plan. Each issue must be addressed separately and verified
through the real UI before it is marked resolved.

## Scope

The current build is technically testable, but it is not yet comfortable or
clear enough for normal pixel-art work. The main problem is not one isolated
control: the interaction model, workspace layout, and region-based editing
model are not visible enough to the user.

## Priority Summary

- **P0 — Foundational:** workspace model, region/cell model, grid semantics,
  palette, and core input behavior.
- **P1 — Workflow blocking:** panels, keybindings, transform/selection
  separation, layers, animation creation, and history controls.
- **P2 — Polish:** zoom feel, labels, discoverability, and interaction
  feedback after the foundational behavior is correct.

## Issues

### ISSUE-001 — Grid is based on pixels instead of regions

**Priority:** P0  
**Area:** Canvas, regions, tiles, grid visualization

The grid currently communicates individual pixel boundaries. In this product,
the primary grid unit is a region/cell, not a single pixel. A user should be
able to understand how the canvas is divided into tile or sprite regions at a
glance.

**User impact:** The canvas looks like a generic pixel editor, and the
region-based workflow is not visually observable.

**Expected behavior:**

- The main grid divides the canvas into region/cell boundaries.
- Pixel-level detail remains available as a secondary view when zoomed in or
  explicitly enabled.
- The current cell/region size and origin are visible and understandable.
- Region boundaries, selection boundaries, and pixel boundaries are visually
  distinguishable.

**Acceptance criteria:** A new user can identify the cell layout, select a
region, and understand its bounds without reading project documentation.

### ISSUE-002 — Workspace panels are not independently dockable

**Priority:** P0  
**Area:** Workspace architecture, layout

Toolbox, layers, color palette, frame editor, preview, and related tools are
currently treated as fixed parts of one compact layout. This prevents users
from arranging the workspace around the task they are performing.

**User impact:** The application feels cramped and unfinished, and important
tools cannot be positioned where they are needed.

**Expected behavior:** Every major workspace feature is an independent,
small dockable panel, including at least:

- Toolbox
- Layers
- Color palette
- Frame editor/timeline
- Preview/playback
- Region/cell information or editor

Panels must be movable, dockable, closable, and restorable without losing the
underlying editor state.

**Acceptance criteria:** A user can rearrange the listed panels, close and
reopen them, and use a useful workspace arrangement for drawing, animation,
and preview tasks.

### ISSUE-003 — Region and cell concepts are not observable

**Priority:** P0  
**Area:** Canvas model, region editor, frame workflow

Regions and cells exist in the data model, but the UI does not make them feel
like first-class objects. The current experience suggests a flat canvas with
some unrelated animation controls.

**User impact:** Users cannot tell what is a region, what is a cell, which
frames reference which region, or how the canvas is organized for export and
animation.

**Expected behavior:** The UI must expose region/cell identity, bounds,
references, and relationships to frames and sequences. Creating, selecting,
renaming, and editing a region must have visible feedback on the canvas and in
the relevant panel.

**Acceptance criteria:** A user can create or select a region, see its
boundary and identity, see which frame uses it, and distinguish region data
from raw canvas pixels.

### ISSUE-004 — There is no clear single color-palette workflow

**Priority:** P0  
**Area:** Color palette

The project should have one shared color palette. The current palette
experience is not functional enough and does not provide a clear way to add
new colors.

**User impact:** Color selection is difficult to manage, and users cannot
build or maintain the palette needed for a pixel-art project.

**Expected behavior:**

- One project palette is the authoritative palette used throughout the app.
- Users can add a new color.
- Users can edit, replace, reorder, and remove colors where the model allows
  it.
- The active color is clearly indicated.
- Palette state is preserved with the project.

**Acceptance criteria:** A user can add a color, select it for drawing, save,
reload, and find the same palette and active-color behavior.

### ISSUE-005 — Keybindings are placed in the wrong workspace location

**Priority:** P1  
**Area:** Settings, navigation, keybindings

Keybindings are currently exposed as a right-side workspace panel. They are a
configuration concern, not a continuously visible editing panel.

**Expected behavior:** Keybindings are accessed from a `Settings` item in the
top bar/menu. They should not occupy permanent right-side workspace space.

**Acceptance criteria:** A user can open Settings from the top bar, edit and
apply keybindings there, and return to the editor without a permanent
keybindings panel.

### ISSUE-006 — Zoom behavior is slow and unintuitive

**Priority:** P1  
**Area:** Canvas navigation, zoom

The current scroll behavior feels dated. Scrolling with Ctrl does not provide
the expected distinction from normal scrolling.

**Expected behavior:**

- Scrolling without Ctrl performs the fast/default zoom behavior.
- Holding Ctrl while scrolling performs the alternate, slower or precise
  zoom behavior.
- Zoom remains anchored at the pointer position.
- The zoom response is consistent across common mouse and trackpad input.

**Acceptance criteria:** A user can quickly move between coarse zoom levels
without modifiers and make controlled zoom adjustments with Ctrl, without the
canvas jumping unexpectedly.

### ISSUE-007 — Ctrl+scroll does not resize the pencil/eraser

**Priority:** P1  
**Area:** Drawing input, brush size

When Pencil or Eraser is active, Ctrl+scroll should change brush size instead
of forcing the user through a separate, slow control.

**Expected behavior:** Ctrl+scroll increases or decreases brush size while
Pencil or Eraser is active. It must not zoom the canvas in that mode.

**Acceptance criteria:** The brush size visibly updates on each supported
scroll step, drawing uses the new size immediately, and other tools retain
their own scroll behavior.

### ISSUE-008 — Middle mouse button is not reserved for panning

**Priority:** P1  
**Area:** Canvas input, navigation

Middle-click currently reaches drawing behavior. It must be a navigation input
only.

**Expected behavior:** Pressing and dragging the middle mouse button activates
pan behavior. It never paints, erases, selects pixels, or changes the active
tool.

**Acceptance criteria:** A middle-button drag changes the canvas view and
leaves pixels, selection state, undo history, and active tool unchanged.

### ISSUE-009 — Transform activation requires awkward Ctrl interaction

**Priority:** P1  
**Area:** Selection and transform workflow

The current transform entry behavior makes users hold Ctrl while interacting.
This is difficult to use for freely rotating, scaling, and skewing a selected
area.

**Expected behavior:** With a selection present, one Ctrl-modified left click
enters transform mode. After entering transform mode, the user should be able
to manipulate the transform handles without continuously holding Ctrl.

**Acceptance criteria:** One Ctrl+left click enters a clearly visible transform
state, and subsequent handle manipulation works without requiring Ctrl to stay
pressed.

### ISSUE-010 — Selection movement and transform behavior are mixed

**Priority:** P1  
**Area:** Selection tool, transform tool

The Selection tool automatically moves the selected area as part of its normal
interaction. Moving, rotating, and scaling pixels should be separate concepts;
transform behavior belongs to the Transform workflow, not ordinary selection.

**Expected behavior:**

- Selection creates, updates, and clears a selection boundary.
- A separate move/transform action explicitly moves or transforms selected
  pixels.
- Selecting an area does not silently commit a pixel move.
- Canceling a transform leaves the document unchanged.

**Acceptance criteria:** A user can select an area without moving pixels, then
explicitly choose move/transform behavior, and undo/redo reflects only the
explicit operation.

### ISSUE-011 — Layer order does not produce reliable compositing order

**Priority:** P1  
**Area:** Layers, compositing, orientation/order controls

Changing layer order does not consistently make the intended layer appear
above or below another layer. The visual result appears detached from the
ordering controls.

**Expected behavior:** The layer list has one unambiguous orientation, and its
order maps directly to bottom-to-top compositing order. Reordering a layer
immediately changes the rendered result.

**Acceptance criteria:** Two overlapping, visibly different layers can be
reordered and the topmost layer visibly wins wherever its pixels are opaque;
undo and redo restore both the order and rendered result.

### ISSUE-012 — Animation creation and editing are undiscoverable

**Priority:** P1  
**Area:** Animation, timeline, frame editor

There is no clear path for creating an animation. The animation area is
currently not usable without already knowing hidden application behavior.

**Expected behavior:** The animation panel exposes an obvious create/new
animation action, frame creation, frame ordering, per-frame duration, loop
state, and playback/preview controls.

**Acceptance criteria:** A new user can create an animation, add at least two
frames, assign regions/cells, change timing, play it, and understand which
frame is currently active.

### ISSUE-013 — Undo and redo are not trustworthy or discoverable

**Priority:** P1  
**Area:** History, command controls

Undo and redo could not be reliably evaluated through the current controls.
Their location, enabled state, and effect are not sufficiently clear.

**Expected behavior:** Undo and redo controls are visible in the top bar or a
clear history area, show disabled/enabled state, and apply to every user-visible
editing operation that claims to be undoable.

**Acceptance criteria:** A user can draw, change a layer, move/transform a
selection, and reorder frames, then undo and redo each operation with a
visibly correct document state.

### ISSUE-014 — Core controls lack coherent product-level feedback

**Priority:** P2  
**Area:** General UX, discoverability, interaction feedback

The application often changes internal state without making the result clear
enough on screen. This contributes to the feeling that controls are
non-functional even when an internal state change occurred.

**Expected behavior:** Active tools, selected regions, current cells, current
frame, active layer, palette color, transform state, zoom level, and pending
history state are all visibly indicated near the relevant control.

**Acceptance criteria:** A user can identify the current editing context from
the UI alone and does not need to infer state from subtle canvas changes.

## Suggested Resolution Order

1. Establish the dockable workspace and top-bar navigation model.
2. Make regions/cells and region-based grid behavior visible.
3. Implement the single editable project palette.
4. Separate selection, movement, and transform workflows.
5. Correct middle-button, zoom, brush-size, and transform input behavior.
6. Verify layer compositing order and history controls.
7. Make animation creation and playback discoverable.
8. Add the remaining feedback and polish improvements.
