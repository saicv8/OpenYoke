// Pure-Tauri frontend: talk to the Rust backend via invoke() instead of HTTP.
const { invoke, Channel } = window.__TAURI__.core;

// --- Sidebar / settings elements --------------------------------------------
const baseUrlInput = document.getElementById('base-url');
const catalogUrlInput = document.getElementById('catalog-url');
const storagePathInput = document.getElementById('storage-path');
const changeStorageButton = document.getElementById('change-storage');
const modelSelect = document.getElementById('model-select');
const refreshButton = document.getElementById('refresh-models');
const saveSettingsButton = document.getElementById('save-settings');
const newConversationButton = document.getElementById('new-conversation');
const statusDiv = document.getElementById('status');
const conversationList = document.getElementById('conversations');

// --- Models tab elements ----------------------------------------------------
const installedModelsDiv = document.getElementById('installed-models');
const pullForm = document.getElementById('pull-form');
const pullNameInput = document.getElementById('pull-name');
const pullProgress = document.getElementById('pull-progress');
const pullLabel = document.getElementById('pull-label');
const pullBar = document.getElementById('pull-bar');
const catalogDiv = document.getElementById('catalog');
const catalogSource = document.getElementById('catalog-source');
const catalogSearch = document.getElementById('catalog-search');

// --- Graph elements ---------------------------------------------------------
const viewport = document.getElementById('graph-viewport');
const world = document.getElementById('graph-world');
const edgesSvg = document.getElementById('graph-edges');
const panelTitle = document.getElementById('panel-title');
const transcript = document.getElementById('panel-transcript');
const nodeForm = document.getElementById('node-form');
const nodePrompt = document.getElementById('node-prompt');
const nodeSend = document.getElementById('node-send');
const nodeDelete = document.getElementById('node-delete');
const newThreadButton = document.getElementById('new-thread');
const graphView = document.getElementById('view-graph');
const nodePanel = document.getElementById('node-panel');
const panelResizer = document.getElementById('panel-resizer');

// --- Storage modal elements -------------------------------------------------
const storageModal = document.getElementById('storage-modal');
const storageInput = document.getElementById('storage-input');
const storageError = document.getElementById('storage-error');
const storageSaveButton = document.getElementById('storage-save');

// --- Shared state -----------------------------------------------------------
let currentConversationId = null;
let conversationsCache = [];
let installedNames = new Set();

function baseUrl() {
  return baseUrlInput.value.trim();
}

function truncate(text, max) {
  const clean = (text || '').replace(/\s+/g, ' ').trim();
  return clean.length > max ? `${clean.slice(0, max)}…` : clean;
}

// --- Minimal, self-contained markdown renderer ------------------------------
// No bundler/CDN here, so this is hand-rolled. It ESCAPES all HTML first, then
// applies markdown, so model output can never inject markup.

function escapeHtml(s) {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function renderInline(text) {
  return text
    .replace(/`([^`]+)`/g, (_, c) => `<code>${c}</code>`)
    .replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (_, t, url) =>
      `<a href="${url}" target="_blank" rel="noopener noreferrer">${t}</a>`)
    .replace(/\*\*([^*]+?)\*\*/g, '<strong>$1</strong>')
    .replace(/\*([^*\n]+?)\*/g, '<em>$1</em>');
}

function renderMarkdown(src) {
  if (!src) return '';

  // 1. Pull out fenced code blocks so their contents aren't marked up.
  const codeBlocks = [];
  let text = src.replace(/```(\w*)\n?([\s\S]*?)```/g, (_, _lang, code) => {
    codeBlocks.push(`<pre><code>${escapeHtml(code.replace(/\n$/, ''))}</code></pre>`);
    return `@CODE${codeBlocks.length - 1}@`;
  });

  // 2. Escape everything else, then apply inline markdown.
  text = renderInline(escapeHtml(text));

  // 3. Block parse line-by-line (paragraphs, headings, lists).
  const out = [];
  let para = [];
  let listType = null;
  const flushPara = () => {
    if (para.length) {
      out.push(`<p>${para.join(' ')}</p>`);
      para = [];
    }
  };
  const closeList = () => {
    if (listType) {
      out.push(`</${listType}>`);
      listType = null;
    }
  };

  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (!line) {
      flushPara();
      closeList();
      continue;
    }
    if (/^@CODE\d+@$/.test(line)) {
      flushPara();
      closeList();
      out.push(line);
      continue;
    }
    const heading = line.match(/^(#{1,6})\s+(.*)$/);
    if (heading) {
      flushPara();
      closeList();
      const level = Math.min(3, heading[1].length); // cap at h3 for the panel
      out.push(`<h${level}>${heading[2]}</h${level}>`);
      continue;
    }
    const ordered = line.match(/^\d+\.\s+(.*)$/);
    const unordered = line.match(/^[-*]\s+(.*)$/);
    if (ordered || unordered) {
      flushPara();
      const type = ordered ? 'ol' : 'ul';
      if (listType !== type) {
        closeList();
        out.push(`<${type}>`);
        listType = type;
      }
      out.push(`<li>${(ordered || unordered)[1]}</li>`);
      continue;
    }
    closeList();
    para.push(line);
  }
  flushPara();
  closeList();

  // 4. Restore code blocks.
  return out.join('\n').replace(/@CODE(\d+)@/g, (_, i) => codeBlocks[Number(i)]);
}

// ============================================================================
// Storage gate (required before anything else loads)
// ============================================================================

function showStorageModal(status) {
  storageInput.value = status.path || '';
  storageError.textContent = status.message || '';
  storageModal.classList.remove('hidden');
  storageInput.focus();
}

function hideStorageModal() {
  storageModal.classList.add('hidden');
}

async function saveStorageDir() {
  const path = storageInput.value.trim();
  storageError.textContent = '';
  try {
    const saved = await invoke('set_storage_dir', { path });
    storagePathInput.value = saved;
    hideStorageModal();
    await loadEverything();
  } catch (error) {
    storageError.textContent = String(error);
  }
}

async function boot() {
  let status;
  try {
    status = await invoke('get_storage_status');
  } catch (error) {
    statusDiv.textContent = `Storage check failed: ${error}`;
    return;
  }
  if (!status.configured || !status.valid) {
    showStorageModal(status);
    return;
  }
  storagePathInput.value = status.path;
  await loadEverything();
}

async function loadEverything() {
  await loadSettings();
  await fetchModels();
  await loadConversations();
}

// ============================================================================
// View tabs
// ============================================================================

document.querySelectorAll('.tab').forEach((tab) => {
  tab.addEventListener('click', () => {
    document.querySelectorAll('.tab').forEach((t) => t.classList.remove('active'));
    tab.classList.add('active');
    const view = tab.dataset.view;
    document.getElementById('view-graph').classList.toggle('hidden', view !== 'graph');
    document.getElementById('view-models').classList.toggle('hidden', view !== 'models');
    if (view === 'graph') {
      fitViewport();
    } else if (view === 'models') {
      renderInstalledModels();
      renderCatalog();
    }
  });
});

// ============================================================================
// Settings
// ============================================================================

async function loadSettings() {
  try {
    const settings = await invoke('load_settings');
    baseUrlInput.value = settings.base_url || 'http://127.0.0.1:11434';
    catalogUrlInput.value = settings.catalog_url || '';
    if (settings.default_model) {
      modelSelect.value = settings.default_model;
    }
  } catch (error) {
    statusDiv.textContent = `Could not load settings: ${error}`;
  }
}

async function saveSettings() {
  try {
    await invoke('save_settings', {
      baseUrl: baseUrl(),
      defaultModel: modelSelect.value || '',
      catalogUrl: catalogUrlInput.value.trim(),
    });
    statusDiv.textContent = 'Settings saved.';
  } catch (error) {
    statusDiv.textContent = `Could not save settings: ${error}`;
  }
}

// ============================================================================
// Conversations (each conversation is a tree of nodes)
// ============================================================================

async function loadConversations() {
  try {
    const data = await invoke('list_conversations');
    conversationsCache = (data.conversations || []).map((c) => ({ ...c, nodes: c.nodes || [] }));
    renderConversationList();
    if (conversationsCache.length) {
      openConversation(conversationsCache[conversationsCache.length - 1].id);
    } else {
      await createConversation();
    }
  } catch (error) {
    statusDiv.textContent = `Could not load conversations: ${error}`;
  }
}

function renderConversationList() {
  conversationList.innerHTML = '';
  conversationsCache.forEach((conversation) => {
    const item = document.createElement('button');
    item.className = 'conversation-item';
    if (conversation.id === currentConversationId) item.classList.add('active');
    item.textContent = conversation.title;
    item.addEventListener('click', () => openConversation(conversation.id));
    conversationList.appendChild(item);
  });
}

async function createConversation() {
  try {
    const conversation = await invoke('create_conversation', { title: 'New conversation' });
    conversation.nodes = conversation.nodes || [];
    conversationsCache.push(conversation);
    openConversation(conversation.id);
    statusDiv.textContent = `Conversation ${conversation.id} created.`;
    return conversation;
  } catch (error) {
    statusDiv.textContent = `Could not create conversation: ${error}`;
    return null;
  }
}

/// The `titleFrom` helper used to name a conversation after its first question.
function titleFrom(question) {
  return truncate(question, 40) || 'New conversation';
}

// ============================================================================
// Graph engine (free-drag canvas, pan/zoom, curved edges)
// ============================================================================

const CARD_W = 240;
const CARD_H = 120;
const GAP_X = 40;
const GAP_Y = 90;
const ZOOM_MIN = 0.3;
const ZOOM_MAX = 2.2;
const DRAG_THRESHOLD2 = 16; // 4px squared

let panX = 0;
let panY = 0;
let zoom = 1;

let nodeIndex = new Map(); // id -> node object (active conversation only)
let cardById = new Map(); // id -> HTMLElement
let edgeByChild = new Map(); // childId -> <path>
let edgesByParent = new Map(); // parentId -> [<path>]
let selectedNodeId = null; // null = virtual root (new thread)
let generating = false;

// Gesture state
let gesture = null; // 'pan' | 'drag' | null
let dragNodeId = null;
let startScreen = { x: 0, y: 0 };
let startPan = { x: 0, y: 0 };
let startNodePos = { x: 0, y: 0 };
let moved = false;
let rafPending = false;
let lastMove = null;

const SVG_NS = 'http://www.w3.org/2000/svg';

function applyTransform() {
  world.style.transform = `translate(${panX}px, ${panY}px) scale(${zoom})`;
}

function cardHeight(id) {
  // offsetHeight is layout px (unaffected by the world's CSS transform), which
  // equals world px. Cards are variable-height, so measure rather than assume.
  const card = cardById.get(id);
  return card ? card.offsetHeight : CARD_H;
}

function edgePath(parent, child) {
  const sx = parent.x + CARD_W / 2;
  const sy = parent.y + cardHeight(parent.id); // real bottom of the parent card
  const ex = child.x + CARD_W / 2;
  const ey = child.y;
  const k = Math.max(40, Math.abs(ey - sy) * 0.5);
  return `M ${sx} ${sy} C ${sx} ${sy + k}, ${ex} ${ey - k}, ${ex} ${ey}`;
}

function addNodeCard(node) {
  nodeIndex.set(node.id, node);

  const card = document.createElement('div');
  card.className = 'gnode';
  card.dataset.nodeId = String(node.id);
  card.style.left = `${node.x}px`;
  card.style.top = `${node.y}px`;
  if (node.id === selectedNodeId) card.classList.add('selected');

  const branch = document.createElement('button');
  branch.className = 'node-branch';
  branch.type = 'button';
  branch.textContent = '+';
  branch.title = 'Branch a new question from here';

  const q = document.createElement('div');
  q.className = 'gnode-q';
  q.textContent = truncate(node.question, 90) || '(empty question)';

  const a = document.createElement('div');
  a.className = 'gnode-a';
  a.textContent = truncate(node.answer, 140) || '…';

  card.appendChild(branch);
  card.appendChild(q);
  card.appendChild(a);
  world.appendChild(card);
  cardById.set(node.id, card);
  return card;
}

function drawEdge(parent, child) {
  const path = document.createElementNS(SVG_NS, 'path');
  path.setAttribute('class', 'gedge');
  path.dataset.parent = String(parent.id);
  path.dataset.child = String(child.id);
  path.setAttribute('d', edgePath(parent, child));
  edgesSvg.appendChild(path);
  edgeByChild.set(child.id, path);
  if (!edgesByParent.has(parent.id)) edgesByParent.set(parent.id, []);
  edgesByParent.get(parent.id).push(path);
}

function redrawEdgesFor(nodeId) {
  const node = nodeIndex.get(nodeId);
  if (!node) return;
  const incoming = edgeByChild.get(nodeId);
  if (incoming) {
    const parent = nodeIndex.get(Number(incoming.dataset.parent));
    if (parent) incoming.setAttribute('d', edgePath(parent, node));
  }
  (edgesByParent.get(nodeId) || []).forEach((path) => {
    const child = nodeIndex.get(Number(path.dataset.child));
    if (child) path.setAttribute('d', edgePath(node, child));
  });
}

function renderGraph() {
  cardById.forEach((card) => card.remove());
  cardById.clear();
  edgeByChild.clear();
  edgesByParent.clear();
  edgesSvg.querySelectorAll('.gedge').forEach((p) => p.remove());

  nodeIndex.forEach((node) => addNodeCard(node));
  nodeIndex.forEach((node) => {
    if (node.parentId != null) {
      const parent = nodeIndex.get(node.parentId);
      if (parent) drawEdge(parent, node);
    }
  });
}

function fitViewport() {
  const rect = viewport.getBoundingClientRect();
  if (!rect.width) return;
  const nodes = [...nodeIndex.values()];
  if (!nodes.length) {
    zoom = 1;
    panX = rect.width / 2 - CARD_W / 2;
    panY = 60;
    applyTransform();
    return;
  }
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  nodes.forEach((n) => {
    minX = Math.min(minX, n.x);
    minY = Math.min(minY, n.y);
    maxX = Math.max(maxX, n.x + CARD_W);
    maxY = Math.max(maxY, n.y + CARD_H);
  });
  const w = Math.max(1, maxX - minX);
  const h = Math.max(1, maxY - minY);
  const pad = 60;
  zoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.min((rect.width - 2 * pad) / w, (rect.height - 2 * pad) / h, 1)));
  panX = (rect.width - w * zoom) / 2 - minX * zoom;
  panY = (rect.height - h * zoom) / 2 - minY * zoom;
  applyTransform();
}

// --- Selection + side panel -------------------------------------------------

function displayPath(id) {
  const chain = [];
  const seen = new Set();
  let current = id;
  let steps = 0;
  while (current != null && steps <= nodeIndex.size) {
    if (seen.has(current)) break;
    seen.add(current);
    const node = nodeIndex.get(current);
    if (!node) break;
    chain.push(node);
    current = node.parentId;
    steps += 1;
  }
  chain.reverse();
  return chain;
}

function bubble(text, role) {
  const node = document.createElement('div');
  node.className = `message ${role}`;
  // Assistant answers are markdown; render them. User text stays literal.
  if (role === 'assistant' && text) node.innerHTML = renderMarkdown(text);
  else node.textContent = text || '(empty)';
  return node;
}

function renderPanel() {
  transcript.innerHTML = '';
  nodeDelete.classList.toggle('hidden', selectedNodeId == null);

  if (selectedNodeId == null) {
    panelTitle.textContent = 'New thread';
    const hint = document.createElement('div');
    hint.className = 'panel-hint';
    hint.textContent = 'Starting a new thread from the root. Ask a question to create the first node.';
    transcript.appendChild(hint);
    return;
  }

  const node = nodeIndex.get(selectedNodeId);
  panelTitle.textContent = truncate(node ? node.question : 'Node', 40) || 'Node';
  displayPath(selectedNodeId).forEach((n) => {
    transcript.appendChild(bubble(n.question, 'user'));
    transcript.appendChild(bubble(n.answer, 'assistant'));
  });
  transcript.scrollTop = transcript.scrollHeight;
}

function selectNode(id) {
  if (selectedNodeId != null) cardById.get(selectedNodeId)?.classList.remove('selected');
  selectedNodeId = id;
  if (id != null) cardById.get(id)?.classList.add('selected');
  renderPanel();
}

// --- Load / render a conversation's tree ------------------------------------

function openConversation(id) {
  currentConversationId = id;
  const conversation = conversationsCache.find((c) => c.id === id);
  nodeIndex = new Map();
  (conversation && Array.isArray(conversation.nodes) ? conversation.nodes : []).forEach((n) => {
    nodeIndex.set(n.id, n);
  });
  selectedNodeId = null;
  renderGraph();
  fitViewport();
  renderPanel();
  renderConversationList(); // the active highlight already shows what's selected
}

// --- Cache patching (nodes never go through syncConversation) ---------------

function patchNode(conversationId, node) {
  const conversation = conversationsCache.find((c) => c.id === conversationId);
  if (conversation) {
    if (!Array.isArray(conversation.nodes)) conversation.nodes = [];
    conversation.nodes.push(node);
  }
}

function pruneNodes(conversationId, removedIds) {
  const set = new Set(removedIds);
  const conversation = conversationsCache.find((c) => c.id === conversationId);
  if (conversation && Array.isArray(conversation.nodes)) {
    conversation.nodes = conversation.nodes.filter((n) => !set.has(n.id));
  }
  if (conversationId === currentConversationId) {
    removedIds.forEach((id) => nodeIndex.delete(id));
  }
}

/// Replace the cached copy of a whole conversation (rename/create only).
function syncConversation(updated) {
  const index = conversationsCache.findIndex((c) => c.id === updated.id);
  const withNodes = { ...updated, nodes: updated.nodes || (conversationsCache[index]?.nodes ?? []) };
  if (index >= 0) conversationsCache[index] = withNodes;
  else conversationsCache.push(withNodes);
}

// --- Placement for a new child ----------------------------------------------

function placeChild(parentId) {
  if (parentId == null) {
    const roots = [...nodeIndex.values()].filter((n) => n.parentId == null);
    return { x: roots.length * (CARD_W + GAP_X), y: 0 };
  }
  const parent = nodeIndex.get(parentId);
  const siblings = [...nodeIndex.values()].filter((n) => n.parentId === parentId);
  return {
    x: (parent ? parent.x : 0) + siblings.length * (CARD_W + GAP_X),
    y: (parent ? parent.y : 0) + CARD_H + GAP_Y,
  };
}

// --- Ask next (create a child of the selected node) -------------------------

async function askNext(question) {
  if (generating) return;
  question = question.trim();
  if (!question) return;

  if (currentConversationId == null) {
    const created = await createConversation();
    if (!created) return;
  }

  const convId = currentConversationId;
  const parentId = selectedNodeId; // may be null (virtual root)
  const model = modelSelect.value;
  const wasEmpty = nodeIndex.size === 0;
  const pos = placeChild(parentId);

  generating = true;
  nodeSend.disabled = true;
  nodePrompt.disabled = true;
  nodeDelete.disabled = true;
  statusDiv.textContent = 'Generating…';

  // Live transcript: drop any "new thread" hint, show the question, and a
  // streaming assistant bubble that fills in as tokens arrive.
  transcript.querySelector('.panel-hint')?.remove();
  transcript.appendChild(bubble(question, 'user'));
  const streamBubble = document.createElement('div');
  streamBubble.className = 'message assistant';
  streamBubble.textContent = '…';
  transcript.appendChild(streamBubble);
  transcript.scrollTop = transcript.scrollHeight;

  // Accumulate tokens and repaint at most once per frame (markdown live-renders;
  // partial markdown just shows literally until it closes).
  let acc = '';
  let renderQueued = false;
  const channel = new Channel();
  channel.onmessage = (chunk) => {
    if (chunk.content) acc += chunk.content;
    if (renderQueued) return;
    renderQueued = true;
    requestAnimationFrame(() => {
      renderQueued = false;
      streamBubble.innerHTML = renderMarkdown(acc) || '…';
      transcript.scrollTop = transcript.scrollHeight;
    });
  };

  try {
    const node = await invoke('create_node', {
      baseUrl: baseUrl(),
      conversationId: convId,
      parentId,
      question,
      model,
      x: pos.x,
      y: pos.y,
      channel,
    });

    patchNode(convId, node);

    // Only touch the canvas if the user hasn't switched conversations.
    if (convId === currentConversationId) {
      addNodeCard(node);
      if (node.parentId != null) {
        const parent = nodeIndex.get(node.parentId);
        if (parent) drawEdge(parent, node);
      }
      selectNode(node.id);
    }

    if (wasEmpty) {
      const renamed = await invoke('rename_conversation', {
        conversationId: convId,
        title: titleFrom(question),
      });
      syncConversation(renamed);
      renderConversationList();
    }

    nodePrompt.value = '';
    statusDiv.textContent = 'Done.';
  } catch (error) {
    // Keep the question in the box so the user can retry; roll the transcript
    // back to its pre-ask state (drops the half-streamed bubble).
    statusDiv.textContent = `Could not generate: ${error}`;
    if (convId === currentConversationId) renderPanel();
  } finally {
    generating = false;
    nodeSend.disabled = false;
    nodePrompt.disabled = false;
    nodeDelete.disabled = false;
  }
}

async function deleteSelectedNode() {
  if (generating) return; // don't race a delete against an in-flight create_node
  if (selectedNodeId == null) return;
  const node = nodeIndex.get(selectedNodeId);
  if (!node) return;
  if (!confirm('Delete this node and all of its follow-ups?')) return;

  const convId = currentConversationId;
  const parentId = node.parentId;
  try {
    const result = await invoke('delete_node', { conversationId: convId, nodeId: selectedNodeId });
    pruneNodes(convId, result.removedIds);
    if (convId === currentConversationId) {
      selectedNodeId = null;
      renderGraph();
      selectNode(parentId != null && nodeIndex.has(parentId) ? parentId : null);
    }
    statusDiv.textContent = `Deleted ${result.removedIds.length} node(s).`;
  } catch (error) {
    statusDiv.textContent = `Could not delete node: ${error}`;
  }
}

// --- Gestures: pan / zoom / drag --------------------------------------------

function beginPan(e) {
  gesture = 'pan';
  moved = false;
  startScreen = { x: e.clientX, y: e.clientY };
  startPan = { x: panX, y: panY };
}

function startNodeDrag(card, e) {
  gesture = 'drag';
  moved = false;
  dragNodeId = Number(card.dataset.nodeId);
  const node = nodeIndex.get(dragNodeId);
  startScreen = { x: e.clientX, y: e.clientY };
  startNodePos = { x: node.x, y: node.y };
  card.classList.add('dragging');
}

function onWorldMouseDown(e) {
  if (e.button !== 0) return;
  if (e.target.closest('.node-branch')) return; // handled by the click listener
  const card = e.target.closest('.gnode');
  if (card) startNodeDrag(card, e);
  else beginPan(e);
  e.preventDefault(); // suppress text selection while dragging
}

function processMove() {
  rafPending = false;
  const e = lastMove;
  if (!e || !gesture) return;
  const dx = e.clientX - startScreen.x;
  const dy = e.clientY - startScreen.y;
  if (dx * dx + dy * dy > DRAG_THRESHOLD2) moved = true;

  if (gesture === 'pan') {
    panX = startPan.x + dx; // screen px (translate is outside scale)
    panY = startPan.y + dy;
    applyTransform();
  } else if (gesture === 'drag') {
    const node = nodeIndex.get(dragNodeId);
    if (!node) return;
    node.x = startNodePos.x + dx / zoom; // world px (cards live inside scale)
    node.y = startNodePos.y + dy / zoom;
    const card = cardById.get(dragNodeId);
    if (card) {
      card.style.left = `${node.x}px`;
      card.style.top = `${node.y}px`;
    }
    redrawEdgesFor(dragNodeId);
  }
}

function onWindowMouseMove(e) {
  if (!gesture) return;
  lastMove = e;
  if (rafPending) return;
  rafPending = true;
  requestAnimationFrame(processMove);
}

function onWindowMouseUp() {
  if (!gesture) return;
  const g = gesture;
  const wasMoved = moved;
  const id = dragNodeId;
  const prev = { ...startNodePos };
  gesture = null;
  dragNodeId = null;
  moved = false;

  if (g === 'drag') {
    cardById.get(id)?.classList.remove('dragging');
    const node = nodeIndex.get(id);
    if (wasMoved && node) {
      const convId = currentConversationId;
      invoke('update_node_position', {
        conversationId: convId,
        nodeId: id,
        x: node.x,
        y: node.y,
      }).catch((error) => {
        node.x = prev.x; // the node object belongs to its own conversation's cache
        node.y = prev.y;
        // Only touch the canvas if that conversation is still the one on screen.
        if (convId === currentConversationId) {
          const card = cardById.get(id);
          if (card) {
            card.style.left = `${node.x}px`;
            card.style.top = `${node.y}px`;
          }
          redrawEdgesFor(id);
        }
        statusDiv.textContent = `Could not save position: ${error}`;
      });
    } else if (!wasMoved) {
      selectNode(id);
    }
  } else if (g === 'pan' && !wasMoved) {
    selectNode(null); // click on empty canvas = new thread
  }
}

function onWheel(e) {
  e.preventDefault();
  const rect = viewport.getBoundingClientRect();
  const cx = e.clientX - rect.left;
  const cy = e.clientY - rect.top;
  const factor = Math.exp(-e.deltaY * 0.0015);
  const newZoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, zoom * factor));
  if (newZoom === zoom) return;
  // Keep the world point under the cursor fixed.
  panX = cx - (cx - panX) * (newZoom / zoom);
  panY = cy - (cy - panY) * (newZoom / zoom);
  zoom = newZoom;
  applyTransform();
}

// Bind gestures to #graph-viewport, not #graph-world: the world collapses to a
// 0x0 box (all children are absolutely positioned), so events on empty canvas
// never reach it. The viewport is the real hit surface; card/branch detection
// still works via e.target.closest() since cards are descendants.
viewport.addEventListener('mousedown', onWorldMouseDown);
window.addEventListener('mousemove', onWindowMouseMove);
window.addEventListener('mouseup', onWindowMouseUp);
viewport.addEventListener('wheel', onWheel, { passive: false });
viewport.addEventListener('click', (e) => {
  const branch = e.target.closest('.node-branch');
  if (!branch) return;
  const id = Number(branch.closest('.gnode').dataset.nodeId);
  selectNode(id);
  nodePrompt.focus();
});

// --- Resizable side panel ---------------------------------------------------

const PANEL_MIN = 280;
const PANEL_WIDTH_KEY = 'openyoke.panelWidth';
let resizing = false;
let resizeStartX = 0;
let resizeStartWidth = 0;

function clampPanelWidth(px) {
  const max = Math.max(PANEL_MIN, Math.min(window.innerWidth * 0.6, 800));
  return Math.min(max, Math.max(PANEL_MIN, px));
}

function setPanelWidth(px) {
  graphView.style.setProperty('--panel-width', `${clampPanelWidth(px)}px`);
}

panelResizer.addEventListener('mousedown', (e) => {
  resizing = true;
  resizeStartX = e.clientX;
  resizeStartWidth = nodePanel.getBoundingClientRect().width;
  panelResizer.classList.add('dragging');
  document.body.style.userSelect = 'none';
  e.preventDefault();
});

window.addEventListener('mousemove', (e) => {
  if (!resizing) return;
  // The panel is on the right, so dragging the divider left widens it.
  setPanelWidth(resizeStartWidth + (resizeStartX - e.clientX));
});

window.addEventListener('mouseup', () => {
  if (!resizing) return;
  resizing = false;
  panelResizer.classList.remove('dragging');
  document.body.style.userSelect = '';
  localStorage.setItem(PANEL_WIDTH_KEY, String(Math.round(nodePanel.getBoundingClientRect().width)));
});

// Restore the saved panel width on load.
const savedPanelWidth = Number(localStorage.getItem(PANEL_WIDTH_KEY));
if (savedPanelWidth) setPanelWidth(savedPanelWidth);

// ============================================================================
// Models tab (unchanged behaviour)
// ============================================================================

async function fetchModels() {
  statusDiv.textContent = 'Loading models...';
  try {
    const data = await invoke('list_models', { baseUrl: baseUrl() });
    const models = data.models || [];
    installedNames = new Set(models.map((m) => m.name));

    const previous = modelSelect.value;
    modelSelect.innerHTML = '';
    models.forEach((model) => {
      const option = document.createElement('option');
      option.value = model.name;
      option.textContent = model.name;
      modelSelect.appendChild(option);
    });
    if (previous && installedNames.has(previous)) modelSelect.value = previous;

    statusDiv.textContent = models.length
      ? `Loaded ${models.length} models.`
      : data.message || 'No models found.';
  } catch (error) {
    statusDiv.textContent = `Could not load models: ${error}`;
  }
}

function formatBytes(bytes) {
  if (!bytes) return '';
  const gb = bytes / 1e9;
  return gb >= 1 ? `${gb.toFixed(1)} GB` : `${(bytes / 1e6).toFixed(0)} MB`;
}

async function renderInstalledModels() {
  await fetchModels();
  installedModelsDiv.innerHTML = '';
  if (!installedNames.size) {
    installedModelsDiv.innerHTML = '<p class="meta">No models installed yet. Download one below.</p>';
    return;
  }
  const data = await invoke('list_models', { baseUrl: baseUrl() });
  (data.models || []).forEach((model) => {
    const item = document.createElement('div');
    item.className = 'installed-item';

    const info = document.createElement('div');
    const name = document.createElement('div');
    name.textContent = model.name;
    const meta = document.createElement('div');
    meta.className = 'meta';
    meta.textContent = formatBytes(model.size);
    info.appendChild(name);
    info.appendChild(meta);

    const del = document.createElement('button');
    del.className = 'danger';
    del.textContent = 'Delete';
    del.addEventListener('click', () => deleteModel(model.name));

    item.appendChild(info);
    item.appendChild(del);
    installedModelsDiv.appendChild(item);
  });
}

async function deleteModel(name) {
  if (!confirm(`Delete ${name}? This removes the downloaded weights.`)) return;
  try {
    await invoke('delete_model', { baseUrl: baseUrl(), model: name });
    statusDiv.textContent = `Deleted ${name}.`;
    await renderInstalledModels();
    await renderCatalog();
  } catch (error) {
    statusDiv.textContent = `Could not delete ${name}: ${error}`;
  }
}

function showPullProgress(label, percent) {
  pullProgress.classList.remove('hidden');
  pullLabel.textContent = label;
  if (percent == null) pullBar.removeAttribute('value');
  else pullBar.value = percent;
}

async function pullModel(model) {
  if (!model) return;
  showPullProgress(`Starting ${model}...`, null);
  const channel = new Channel();
  channel.onmessage = (msg) => {
    if (msg.error) {
      showPullProgress(`Error: ${msg.error}`, null);
      return;
    }
    let percent = null;
    if (msg.total && msg.completed != null) percent = Math.floor((msg.completed / msg.total) * 100);
    const suffix = percent != null ? ` — ${percent}%` : '';
    showPullProgress(msg.done ? `${model} ready.` : `${msg.status}${suffix}`, percent);
  };
  try {
    await invoke('pull_model', { baseUrl: baseUrl(), model, channel });
    showPullProgress(`${model} downloaded.`, 100);
    await renderInstalledModels();
    await renderCatalog();
    statusDiv.textContent = `Downloaded ${model}.`;
  } catch (error) {
    showPullProgress(`Error: ${error}`, null);
  }
}

pullForm.addEventListener('submit', (event) => {
  event.preventDefault();
  const model = pullNameInput.value.trim();
  if (model) {
    pullModel(model);
    pullNameInput.value = '';
  }
});

let catalogModels = [];

async function renderCatalog() {
  catalogDiv.innerHTML = '<p class="meta">Loading library…</p>';
  let data;
  try {
    data = await invoke('fetch_catalog');
  } catch (error) {
    catalogDiv.innerHTML = `<p class="meta">Could not load library: ${error}</p>`;
    return;
  }
  catalogModels = data.models || [];
  catalogSource.textContent =
    data.source === 'live' ? '(live · ollama.com)' : data.source === 'remote' ? '(remote)' : '(bundled)';
  renderCatalogCards(catalogSearch.value.trim());
}

function renderCatalogCards(query) {
  const q = (query || '').toLowerCase();
  const models = q
    ? catalogModels.filter(
        (m) =>
          (m.name || '').toLowerCase().includes(q) ||
          (m.description || '').toLowerCase().includes(q) ||
          (m.tags || []).some((t) => String(t).toLowerCase().includes(q))
      )
    : catalogModels;

  catalogDiv.innerHTML = '';
  if (!models.length) {
    catalogDiv.innerHTML = `<p class="meta">${catalogModels.length ? 'No models match your search.' : 'No models found.'}</p>`;
    return;
  }

  models.forEach((model) => {
    const card = document.createElement('div');
    card.className = 'catalog-card';

    const title = document.createElement('h3');
    title.textContent = model.title || model.name;

    const publisher = document.createElement('div');
    publisher.className = 'publisher';
    publisher.textContent = model.publisher || '';

    const desc = document.createElement('div');
    desc.className = 'desc';
    desc.textContent = model.description || '';

    const chips = document.createElement('div');
    chips.className = 'tag-chips';
    (model.tags || []).forEach((tag) => {
      const chip = document.createElement('span');
      chip.className = 'tag-chip';
      chip.textContent = tag;
      chips.appendChild(chip);
    });

    const variants = document.createElement('div');
    variants.className = 'variant-row';
    (model.variants || []).forEach((variant) => {
      const btn = document.createElement('button');
      const installed = installedNames.has(variant.tag);
      btn.className = installed ? 'variant-btn installed' : 'variant-btn';
      const label = variant.size ? `${variant.label} · ${variant.size}` : variant.label;
      btn.textContent = installed ? `✓ ${variant.label}` : label;
      if (!installed) btn.addEventListener('click', () => pullModel(variant.tag));
      variants.appendChild(btn);
    });

    card.appendChild(title);
    if (model.publisher) card.appendChild(publisher);
    card.appendChild(desc);
    card.appendChild(chips);
    card.appendChild(variants);
    catalogDiv.appendChild(card);
  });
}

catalogSearch.addEventListener('input', () => renderCatalogCards(catalogSearch.value.trim()));

// ============================================================================
// Wire up + boot
// ============================================================================

refreshButton.addEventListener('click', fetchModels);
saveSettingsButton.addEventListener('click', saveSettings);
newConversationButton.addEventListener('click', createConversation);
storageSaveButton.addEventListener('click', saveStorageDir);
storageInput.addEventListener('keydown', (event) => {
  if (event.key === 'Enter') saveStorageDir();
});
changeStorageButton.addEventListener('click', async () => {
  const status = await invoke('get_storage_status');
  showStorageModal(status);
});

nodeForm.addEventListener('submit', (event) => {
  event.preventDefault();
  askNext(nodePrompt.value);
});
nodeDelete.addEventListener('click', deleteSelectedNode);
newThreadButton.addEventListener('click', () => {
  selectNode(null);
  nodePrompt.focus();
});

boot();
