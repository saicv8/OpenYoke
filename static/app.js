// Pure-Tauri frontend: talk to the Rust backend via invoke() instead of HTTP.
const { invoke, Channel } = window.__TAURI__.core;

// --- Sidebar / settings elements --------------------------------------------
const baseUrlInput = document.getElementById('base-url');
const catalogUrlInput = document.getElementById('catalog-url');
const anthropicKeyInput = document.getElementById('anthropic-key');
const openaiKeyInput = document.getElementById('openai-key');
const openaiBaseInput = document.getElementById('openai-base');
const googleKeyInput = document.getElementById('google-key');
const systemPromptInput = document.getElementById('system-prompt');
const storagePathInput = document.getElementById('storage-path');
const changeStorageButton = document.getElementById('change-storage');
const modelSelect = document.getElementById('model-select');
const refreshButton = document.getElementById('refresh-models');
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
const webToggle = document.getElementById('web-toggle');
const graphView = document.getElementById('view-graph');
const nodePanel = document.getElementById('node-panel');
const panelResizer = document.getElementById('panel-resizer');

// --- Storage modal elements -------------------------------------------------
const storageModal = document.getElementById('storage-modal');
const storageInput = document.getElementById('storage-input');
const storageError = document.getElementById('storage-error');
const storageSaveButton = document.getElementById('storage-save');

// --- Confirmation dialog ----------------------------------------------------
const confirmModal = document.getElementById('confirm-modal');
const confirmTitle = document.getElementById('confirm-title');
const confirmMessage = document.getElementById('confirm-message');
const confirmOk = document.getElementById('confirm-ok');
const confirmCancel = document.getElementById('confirm-cancel');
let confirmResolve = null;

/// In-app replacement for window.confirm() (which is unreliable in the webview).
/// Returns a Promise<boolean>.
function confirmDialog(message, { title = 'Are you sure?', confirmLabel = 'Delete' } = {}) {
  confirmTitle.textContent = title;
  confirmMessage.textContent = message;
  confirmOk.textContent = confirmLabel;
  confirmModal.classList.remove('hidden');
  confirmOk.focus();
  return new Promise((resolve) => {
    confirmResolve = resolve;
  });
}

function closeConfirm(result) {
  confirmModal.classList.add('hidden');
  const resolve = confirmResolve;
  confirmResolve = null;
  if (resolve) resolve(result);
}

confirmOk.addEventListener('click', () => closeConfirm(true));
confirmCancel.addEventListener('click', () => closeConfirm(false));
confirmModal.addEventListener('click', (event) => {
  if (event.target === confirmModal) closeConfirm(false); // click backdrop = cancel
});

// --- Settings dialog --------------------------------------------------------
// Every field in here autosaves as it's edited, so opening and closing the
// dialog is free — there's nothing to commit and nothing to discard.
const settingsModal = document.getElementById('settings-modal');
const settingsDialog = settingsModal.querySelector('.modal-settings');
const openSettingsButton = document.getElementById('open-settings');
const closeSettingsButton = document.getElementById('settings-close');
const settingsDoneButton = document.getElementById('settings-done');

function openSettings() {
  settingsModal.classList.remove('hidden');
  settingsDialog.focus(); // land inside the dialog without pre-selecting a field
}

function closeSettings() {
  flushAutosave(); // don't sit on a pending debounce once it's out of sight
  settingsModal.classList.add('hidden');
  openSettingsButton.focus();
}

function settingsOpen() {
  return !settingsModal.classList.contains('hidden');
}

openSettingsButton.addEventListener('click', openSettings);
closeSettingsButton.addEventListener('click', closeSettings);
settingsDoneButton.addEventListener('click', closeSettings);
settingsModal.addEventListener('click', (event) => {
  if (event.target === settingsModal) closeSettings(); // click backdrop = close
});

// Escape closes the topmost dialog. The storage gate is deliberately absent:
// it's required, so there's nothing to escape to.
document.addEventListener('keydown', (event) => {
  if (event.key !== 'Escape') return;
  if (!confirmModal.classList.contains('hidden')) closeConfirm(false);
  else if (settingsOpen()) closeSettings();
});

// --- Shared state -----------------------------------------------------------
let currentConversationId = null;
let conversationsCache = [];
let installedNames = new Set();
let savedDefaultModel = '';
// Set when the saved model is missing at load; surfaced after boot finishes.
let modelWarning = '';
let webSearchOn = true; // web search on by default

function baseUrl() {
  return baseUrlInput.value.trim();
}

function truncate(text, max) {
  const clean = (text || '').replace(/\s+/g, ' ').trim();
  return clean.length > max ? `${clean.slice(0, max)}…` : clean;
}

// Known provider prefixes. A model is stored as `provider:id`; anything without
// one of these prefixes is a bare Ollama id from before providers were tagged.
const PROVIDERS = { ollama: 'Ollama', anthropic: 'Anthropic', openai: 'OpenAI', google: 'Google' };

/// Split a stored `provider:id` into { provider, id } for display. The id itself
/// can contain colons (`ollama:llama3.2:1b`), so only the FIRST segment is ever
/// treated as the provider, and only when it's one we recognise.
function splitModel(model) {
  const raw = (model || '').trim();
  if (!raw) return { provider: '', id: '' };
  const colon = raw.indexOf(':');
  const head = colon === -1 ? raw : raw.slice(0, colon);
  if (colon !== -1 && PROVIDERS[head]) {
    return { provider: head, id: raw.slice(colon + 1) };
  }
  return { provider: 'ollama', id: raw }; // bare id -> Ollama (backend default)
}

/// Short label for a node badge: just the model id (the provider shows as a
/// coloured dot). Empty string when the node predates model tracking.
function modelLabel(model) {
  return splitModel(model).id;
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

  const lines = text.split('\n');
  const isTableSep = (s) => /\|/.test(s) && /-/.test(s) && /^[\s|:-]+$/.test(s.trim());
  const splitRow = (s) => {
    let row = s.trim();
    if (row.startsWith('|')) row = row.slice(1);
    if (row.endsWith('|')) row = row.slice(0, -1);
    return row.split('|').map((c) => c.trim());
  };

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i].trim();
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

    // GFM table: a `| … |` row immediately followed by a `|---|---|` separator.
    if (line.includes('|') && i + 1 < lines.length && isTableSep(lines[i + 1])) {
      flushPara();
      closeList();
      const header = splitRow(line);
      const rows = [];
      let j = i + 2;
      while (j < lines.length && lines[j].trim() && lines[j].includes('|')) {
        rows.push(splitRow(lines[j]));
        j += 1;
      }
      const head = header.map((c) => `<th>${c}</th>`).join('');
      const body = rows
        .map((r) => `<tr>${header.map((_, k) => `<td>${r[k] || ''}</td>`).join('')}</tr>`)
        .join('');
      out.push(`<div class="table-wrap"><table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table></div>`);
      i = j - 1; // skip the rows we consumed
      continue;
    }

    // Horizontal rule.
    if (/^(-{3,}|\*{3,}|_{3,})$/.test(line)) {
      flushPara();
      closeList();
      out.push('<hr>');
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
  // fetchModels' status line gets overwritten by the steps after it, so a
  // warning about the saved model going missing is re-asserted last — it
  // matters more than "Conversation N created."
  if (modelWarning) statusDiv.textContent = modelWarning;
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
    anthropicKeyInput.value = settings.anthropic_key || '';
    openaiKeyInput.value = settings.openai_key || '';
    openaiBaseInput.value = settings.openai_base_url || '';
    googleKeyInput.value = settings.google_key || '';
    systemPromptInput.value = settings.system_prompt || '';
    savedDefaultModel = settings.default_model || '';
    modelSelect.value = savedDefaultModel; // re-applied by fetchModels once options exist
  } catch (error) {
    statusDiv.textContent = `Could not load settings: ${error}`;
  }
}

// --- Settings autosave ------------------------------------------------------
//
// Settings persist as they're edited; there is no Save button. Typing is
// debounced so a write happens once the user pauses rather than on every
// keystroke, and blur/Enter flushes immediately.

const AUTOSAVE_DELAY = 600;
let autosaveTimer = null;
let autosaveFollowUp = {};

async function persistSettings({ reloadModels = false, reloadCatalog = false } = {}) {
  try {
    await invoke('save_settings', {
      baseUrl: baseUrl(),
      // An empty picker (Ollama down, or no keys yet) must not wipe the stored
      // model — autosave fires far more often than the old Save button, so this
      // would otherwise erase the choice on an unrelated keystroke.
      defaultModel: modelSelect.value || savedDefaultModel || '',
      catalogUrl: catalogUrlInput.value.trim(),
      openaiKey: openaiKeyInput.value.trim(),
      openaiBaseUrl: openaiBaseInput.value.trim(),
      anthropicKey: anthropicKeyInput.value.trim(),
      googleKey: googleKeyInput.value.trim(),
      systemPrompt: systemPromptInput.value.trim(),
    });
    savedDefaultModel = modelSelect.value || savedDefaultModel || '';
    statusDiv.textContent = 'Settings saved.';
    // Only re-query providers when a field that changes their result moved.
    if (reloadModels) await fetchModels();
    if (reloadCatalog) await renderCatalog();
  } catch (error) {
    statusDiv.textContent = `Could not save settings: ${error}`;
  }
}

/// Queue a save. Repeated edits collapse into one write, and the reload flags
/// of everything typed during the window are merged so none are dropped.
function scheduleAutosave(options) {
  autosaveFollowUp = { ...autosaveFollowUp, ...options };
  clearTimeout(autosaveTimer);
  autosaveTimer = setTimeout(flushAutosave, AUTOSAVE_DELAY);
}

function flushAutosave() {
  clearTimeout(autosaveTimer);
  autosaveTimer = null;
  const options = autosaveFollowUp;
  autosaveFollowUp = {};
  return persistSettings(options);
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
  // Newest first (ids are monotonic, so highest id = most recently created).
  const ordered = [...conversationsCache].sort((a, b) => b.id - a.id);
  ordered.forEach((conversation) => {
    const item = document.createElement('div');
    item.className = 'conversation-item';
    if (conversation.id === currentConversationId) item.classList.add('active');
    item.addEventListener('click', () => openConversation(conversation.id));

    const title = document.createElement('span');
    title.className = 'conversation-title';
    title.textContent = conversation.title;

    const del = document.createElement('button');
    del.className = 'conversation-delete';
    del.title = 'Delete conversation';
    del.innerHTML =
      '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14M10 11v4M14 11v4"/></svg>';
    del.addEventListener('click', (event) => {
      event.stopPropagation(); // don't open the conversation
      deleteConversation(conversation.id, conversation.title);
    });

    item.appendChild(title);
    item.appendChild(del);
    conversationList.appendChild(item);
  });
}

async function deleteConversation(id, title) {
  const ok = await confirmDialog(
    `This removes “${title}” and all of its branches, and can't be undone.`,
    { title: 'Delete conversation?', confirmLabel: 'Delete' }
  );
  if (!ok) return;
  try {
    await invoke('delete_conversation', { conversationId: id });
    conversationsCache = conversationsCache.filter((c) => c.id !== id);
    if (id === currentConversationId) {
      // The open conversation was deleted — fall back to the newest remaining, or a fresh one.
      if (conversationsCache.length) {
        openConversation(conversationsCache[conversationsCache.length - 1].id);
      } else {
        await createConversation();
      }
    } else {
      renderConversationList();
    }
    statusDiv.textContent = `Deleted "${title}".`;
  } catch (error) {
    statusDiv.textContent = `Could not delete conversation: ${error}`;
  }
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

// Where the next unconnected root should land, set when a new thread is started
// by double-clicking empty canvas so the root appears where the user clicked.
// Null means "lay it out automatically" (the + New thread button's behaviour).
let newRootPos = null;

// A placeholder node shown on the canvas while an answer streams in. Real ids
// are positive (minted by the backend), so a negative id can never collide.
//
// It exists so the in-flight answer is anchored to something VISIBLE. Without
// it the stream lived only in the side panel, and anything that re-rendered the
// panel — clicking empty canvas, for one — detached the bubble mid-stream and
// the answer appeared to vanish.
const PENDING_ID = -1;
let pendingNode = null;

/// Remove the placeholder from the graph. Safe to call when there isn't one.
function clearPendingNode() {
  const edge = edgeByChild.get(PENDING_ID);
  if (edge) {
    const parentId = Number(edge.dataset.parent);
    const siblings = edgesByParent.get(parentId);
    if (siblings) edgesByParent.set(parentId, siblings.filter((p) => p !== edge));
    edge.remove();
    edgeByChild.delete(PENDING_ID);
  }
  cardById.get(PENDING_ID)?.remove();
  cardById.delete(PENDING_ID);
  nodeIndex.delete(PENDING_ID);
  pendingNode = null;
}

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
  const badge = modelBadge(node.model);
  if (badge) card.appendChild(badge);
  world.appendChild(card);
  cardById.set(node.id, card);
  return card;
}

/// A small "● model-id" chip identifying which model produced a node, tinted by
/// provider. Returns null for nodes with no recorded model (pre-tracking data).
function modelBadge(model) {
  const { provider, id } = splitModel(model);
  if (!id) return null;
  const badge = document.createElement('div');
  badge.className = `gnode-model provider-${provider}`;
  badge.title = `Answered by ${PROVIDERS[provider] || provider} · ${id}`;

  const dot = document.createElement('span');
  dot.className = 'model-dot';
  const label = document.createElement('span');
  label.className = 'model-name';
  label.textContent = id;

  badge.appendChild(dot);
  badge.appendChild(label);
  return badge;
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

/// Shown under an answer that stopped early, so a half-written reply can't be
/// mistaken for a complete one. Null for the normal case.
function truncationNote(reason) {
  if (!reason) return null;
  const note = document.createElement('div');
  note.className = 'msg-truncated';
  note.textContent = `This answer is incomplete — ${reason}.`;
  return note;
}

/// A muted "● Provider · model-id" line shown under an answer in the transcript.
/// Null for pre-tracking nodes with no recorded model.
function modelCaption(model) {
  const { provider, id } = splitModel(model);
  if (!id) return null;
  const caption = document.createElement('div');
  caption.className = `msg-model provider-${provider}`;
  const dot = document.createElement('span');
  dot.className = 'model-dot';
  caption.appendChild(dot);
  caption.appendChild(document.createTextNode(`${PROVIDERS[provider] || provider} · ${id}`));
  return caption;
}

function renderPanel() {
  transcript.innerHTML = '';
  nodeDelete.classList.toggle('hidden', selectedNodeId == null);
  // The button only makes sense when a node is selected — it's the way OUT of a
  // thread into a fresh one. With nothing selected you're already composing a
  // new thread, so it would be a no-op.
  newThreadButton.classList.toggle('hidden', selectedNodeId == null);

  if (selectedNodeId == null) {
    panelTitle.textContent = 'New thread';
    const hint = document.createElement('div');
    hint.className = 'panel-hint';
    hint.textContent = 'Starting a new, unconnected thread. Ask a question to create its first node.';
    transcript.appendChild(hint);
    return;
  }

  const node = nodeIndex.get(selectedNodeId);
  panelTitle.textContent = truncate(node ? node.question : 'Node', 40) || 'Node';
  displayPath(selectedNodeId).forEach((n) => {
    transcript.appendChild(bubble(n.question, 'user'));
    const answer = bubble(n.answer, 'assistant');
    // Tag the in-flight bubble so streaming tokens can find it again after any
    // re-render — that's what lets the user click away and back without losing
    // the live answer.
    if (n.id === PENDING_ID) {
      answer.classList.add('streaming');
      if (!n.answer) answer.textContent = '…';
    }
    transcript.appendChild(answer);
    const note = truncationNote(n.truncated);
    if (note) transcript.appendChild(note);
    // Caption each answer with the model that produced it — a thread can mix
    // models turn to turn, so this isn't derivable from the active picker.
    const caption = modelCaption(n.model);
    if (caption) transcript.appendChild(caption);
  });
  transcript.scrollTop = transcript.scrollHeight;
}

function selectNode(id) {
  if (selectedNodeId != null) cardById.get(selectedNodeId)?.classList.remove('selected');
  selectedNodeId = id;
  if (id != null) cardById.get(id)?.classList.add('selected');
  renderPanel();
}

/// Start a new, unconnected thread: deselect so the next question becomes a
/// root node. `pos` (world coords) pins where that root lands — passed when the
/// thread is started by double-clicking empty canvas; null lays it out
/// automatically beside any existing roots.
function startNewThread(pos = null) {
  if (generating) return; // can't retarget an in-flight generation
  newRootPos = pos;
  selectNode(null);
  nodePrompt.focus();
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
  // A root started by double-clicking the canvas lands where it was clicked;
  // otherwise fall back to automatic layout. Consume it either way.
  const pos = parentId == null && newRootPos ? newRootPos : placeChild(parentId);
  newRootPos = null;

  generating = true;
  nodeSend.disabled = true;
  nodePrompt.disabled = true;
  nodeDelete.disabled = true;
  newThreadButton.disabled = true;
  statusDiv.textContent = webSearchOn ? 'Searching the web…' : 'Generating…';

  // Put a placeholder card on the canvas straight away, so the pending answer
  // is visible in the graph rather than living only in the side panel.
  pendingNode = { id: PENDING_ID, parentId, question, answer: '', model, x: pos.x, y: pos.y };
  addNodeCard(pendingNode);
  cardById.get(PENDING_ID)?.classList.add('pending');
  if (parentId != null) {
    const parent = nodeIndex.get(parentId);
    if (parent) drawEdge(parent, pendingNode);
  }
  selectNode(PENDING_ID); // renders the question + an empty streaming bubble

  // Accumulate tokens and repaint at most once per frame (markdown live-renders;
  // partial markdown just shows literally until it closes).
  let acc = '';
  let renderQueued = false;
  const channel = new Channel();
  channel.onmessage = (chunk) => {
    if (chunk.content) acc += chunk.content;
    // Set only on the final chunk, and only when the reply ended early. Shown
    // right away so the user isn't left wondering why the text stopped; the
    // saved node carries the same reason for later.
    if (chunk.error) {
      statusDiv.textContent = `Reply cut off — ${chunk.error}.`;
      if (pendingNode) pendingNode.truncated = chunk.error;
    }
    if (renderQueued) return;
    renderQueued = true;
    requestAnimationFrame(() => {
      renderQueued = false;
      if (!pendingNode) return; // generation ended (or the user switched away)
      pendingNode.answer = acc;

      const preview = cardById.get(PENDING_ID)?.querySelector('.gnode-a');
      if (preview) preview.textContent = truncate(acc, 140) || '…';

      // Re-query rather than holding a reference: renderPanel() rebuilds the
      // transcript, so a captured element can be stale by now.
      const live = transcript.querySelector('.message.assistant.streaming');
      if (live) {
        live.innerHTML = renderMarkdown(acc) || '…';
        transcript.scrollTop = transcript.scrollHeight;
      }
    });
  };

  try {
    const node = await invoke('create_node', {
      baseUrl: baseUrl(),
      conversationId: convId,
      parentId,
      question,
      model,
      webSearch: webSearchOn,
      x: pos.x,
      y: pos.y,
      channel,
    });

    // Carry over a placeholder the user dragged while it was streaming, so the
    // finished card doesn't jump back to where the request started.
    const movedTo = pendingNode && (pendingNode.x !== pos.x || pendingNode.y !== pos.y)
      ? { x: pendingNode.x, y: pendingNode.y }
      : null;
    if (movedTo) {
      node.x = movedTo.x;
      node.y = movedTo.y;
    }
    patchNode(convId, node);

    // Only touch the canvas if the user hasn't switched conversations.
    if (convId === currentConversationId) {
      const wasFollowingPending = selectedNodeId === PENDING_ID;
      clearPendingNode();
      addNodeCard(node);
      if (node.parentId != null) {
        const parent = nodeIndex.get(node.parentId);
        if (parent) drawEdge(parent, node);
      }
      // Follow through to the finished node only if the user was still watching
      // it stream. If they clicked over to another card mid-generation, leave
      // them where they are rather than yanking the panel away.
      if (wasFollowingPending) selectNode(node.id);
      else renderPanel();
    }

    if (movedTo) {
      invoke('update_node_position', { conversationId: convId, nodeId: node.id, ...movedTo })
        .catch((error) => { statusDiv.textContent = `Could not save position: ${error}`; });
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
    // Don't overwrite the cut-off notice with "Done." — it wasn't.
    statusDiv.textContent = node.truncated
      ? `Reply cut off — ${node.truncated}.`
      : 'Done.';
  } catch (error) {
    // Keep the question in the box so the user can retry; roll the transcript
    // back to its pre-ask state (drops the half-streamed bubble).
    statusDiv.textContent = `Could not generate: ${error}`;
    const wasFollowingPending = selectedNodeId === PENDING_ID;
    clearPendingNode();
    if (wasFollowingPending) {
      // Fall back to the node we were branching from (or any surviving node)
      // rather than dropping the user into a blank new-thread they didn't ask
      // for. Starting a new thread is now an explicit action (button /
      // double-click), so this stays a helpful default, not a hard rule.
      selectedNodeId = parentId != null && nodeIndex.has(parentId) ? parentId : null;
      if (selectedNodeId == null && nodeIndex.size > 0) {
        selectedNodeId = Math.max(...nodeIndex.keys());
      }
      cardById.get(selectedNodeId)?.classList.add('selected');
    }
    if (convId === currentConversationId) renderPanel();
  } finally {
    clearPendingNode(); // no-op on the paths that already cleared it
    generating = false;
    nodeSend.disabled = false;
    nodePrompt.disabled = false;
    nodeDelete.disabled = false;
    newThreadButton.disabled = false;
  }
}

async function deleteSelectedNode() {
  if (generating) return; // don't race a delete against an in-flight create_node
  if (selectedNodeId == null) return;
  const node = nodeIndex.get(selectedNodeId);
  if (!node) return;
  const ok = await confirmDialog('This deletes the node and all of its follow-ups.', {
    title: 'Delete node?',
    confirmLabel: 'Delete',
  });
  if (!ok) return;

  const convId = currentConversationId;
  const parentId = node.parentId;
  try {
    const result = await invoke('delete_node', { conversationId: convId, nodeId: selectedNodeId });
    pruneNodes(convId, result.removedIds);
    if (convId === currentConversationId) {
      selectedNodeId = null;
      renderGraph();
      // Land on the parent, else any surviving node. Leaving nothing selected
      // while the canvas still has nodes would re-expose root-level creation,
      // which is only allowed on an empty canvas.
      let next = parentId != null && nodeIndex.has(parentId) ? parentId : null;
      if (next == null && nodeIndex.size > 0) next = Math.max(...nodeIndex.keys());
      selectNode(next);
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
    if (wasMoved && node && id === PENDING_ID) {
      // The placeholder has no backend row yet; its position is persisted once
      // the real node arrives (see askNext).
    } else if (wasMoved && node) {
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
    // Clicking empty canvas is a no-op on selection: it used to deselect, which
    // tore down the panel mid-stream and made the open conversation look lost.
    // The one exception is an empty canvas, where "no selection" is the only
    // state there is and the new-thread hint is what the user needs to see.
    if (nodeIndex.size === 0) selectNode(null);
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

// Double-clicking empty canvas starts a new thread positioned right there.
viewport.addEventListener('dblclick', (e) => {
  if (e.target.closest('.gnode')) return; // a card owns its own double-click
  const rect = viewport.getBoundingClientRect();
  // Screen point -> world coords: translate is applied outside the scale, so
  // undo the pan first, then the zoom. Centre the card under the cursor.
  const worldX = (e.clientX - rect.left - panX) / zoom - CARD_W / 2;
  const worldY = (e.clientY - rect.top - panY) / zoom;
  startNewThread({ x: worldX, y: worldY });
});

newThreadButton.addEventListener('click', () => startNewThread());

// --- Resizable side panel ---------------------------------------------------

// These three mirror --panel-min / --graph-min / --resizer-w in styles.css,
// which enforces the same bounds on the rendered layout.
const PANEL_MIN = 280;
const GRAPH_MIN = 320; // the canvas keeps this much, and the panel gets the rest
const RESIZER_W = 6;
const PANEL_WIDTH_KEY = 'openyoke.panelWidth';
let resizing = false;
let resizeStartX = 0;
let resizeStartWidth = 0;

/// How wide the panel is allowed to get. The only real constraint is leaving a
/// usable canvas, so measure the row the panel actually sits in — that already
/// accounts for the sidebar being open or shut — and hand over everything past
/// GRAPH_MIN. On any normal window this is well over half the width.
function panelMaxWidth() {
  // The Models tab hides the graph view, which measures 0; fall back to the window.
  const row = graphView.getBoundingClientRect().width || window.innerWidth;
  return Math.max(PANEL_MIN, row - RESIZER_W - GRAPH_MIN);
}

function clampPanelWidth(px) {
  return Math.min(panelMaxWidth(), Math.max(PANEL_MIN, px));
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
  // The panel is on the left, so dragging the divider right widens it.
  setPanelWidth(resizeStartWidth + (e.clientX - resizeStartX));
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
  modelWarning = '';
  try {
    const data = await invoke('list_all_models', { baseUrl: baseUrl() });
    const groups = data.groups || [];

    // Ollama models feed the catalog's "installed" checks.
    const ollama = groups.find((g) => g.provider === 'ollama');
    installedNames = new Set((ollama && ollama.models) || []);

    const desired = modelSelect.value || savedDefaultModel;
    modelSelect.innerHTML = '';
    let total = 0;
    groups.forEach((group) => {
      if (!group.models || !group.models.length) return;
      const optgroup = document.createElement('optgroup');
      optgroup.label = group.label;
      group.models.forEach((id) => {
        const option = document.createElement('option');
        option.value = `${group.provider}:${id}`; // e.g. anthropic:claude-…
        option.textContent = id;
        optgroup.appendChild(option);
        total += 1;
      });
      modelSelect.appendChild(optgroup);
    });
    // Restore the saved choice. Assigning a value with no matching <option>
    // silently blanks the select, so check before trusting it.
    if (desired) modelSelect.value = desired;
    const restored = modelSelect.value === desired;
    if (!restored && total) modelSelect.selectedIndex = 0;

    const providers = groups.filter((g) => g.models && g.models.length).length;
    if (!total) {
      statusDiv.textContent = 'No models. Start Ollama or add an API key, then Refresh.';
    } else if (desired && !restored) {
      // Don't fail silently: the previous model is gone (uninstalled, or its
      // provider lost its API key) and we've moved them onto another one.
      modelWarning = `"${desired}" is no longer available — switched to ${modelSelect.value}.`;
      statusDiv.textContent = modelWarning;
      savedDefaultModel = modelSelect.value;
      invoke('set_default_model', { model: modelSelect.value }).catch(() => {});
    } else {
      statusDiv.textContent = `Loaded ${total} models across ${providers} provider(s).`;
    }
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
  const ok = await confirmDialog(`This removes the downloaded weights for ${name}.`, {
    title: 'Delete model?',
    confirmLabel: 'Delete',
  });
  if (!ok) return;
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

// Persist the model as soon as it's picked. Relying on "Save settings" meant a
// switched model was lost on quit, since nothing else writes default_model.
// This uses the single-field command rather than a full save, so picking a
// model can't commit a half-typed API key sitting in another input.
modelSelect.addEventListener('change', async () => {
  savedDefaultModel = modelSelect.value;
  try {
    await invoke('set_default_model', { model: modelSelect.value });
    statusDiv.textContent = 'Settings saved.';
  } catch (error) {
    statusDiv.textContent = `Could not save model choice: ${error}`;
  }
});

// Every text setting autosaves. The flags say which caches a field invalidates:
// provider credentials and the Ollama URL change the model list, the catalog
// URL changes the library.
[
  [baseUrlInput, { reloadModels: true }],
  [anthropicKeyInput, { reloadModels: true }],
  [openaiKeyInput, { reloadModels: true }],
  [openaiBaseInput, { reloadModels: true }],
  [googleKeyInput, { reloadModels: true }],
  [catalogUrlInput, { reloadCatalog: true }],
  [systemPromptInput, {}],
].forEach(([input, options]) => {
  input.addEventListener('input', () => scheduleAutosave(options));
  // Leaving the field (or pressing Enter) shouldn't wait out the debounce.
  input.addEventListener('change', () => {
    scheduleAutosave(options);
    flushAutosave();
  });
});

refreshButton.addEventListener('click', fetchModels);
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
webToggle.addEventListener('click', () => {
  webSearchOn = !webSearchOn;
  webToggle.classList.toggle('active', webSearchOn);
  webToggle.title = webSearchOn
    ? 'Web search is ON — results are searched and used in the answer'
    : 'Search the web and use the results in the answer (works with any model)';
});
webToggle.classList.toggle('active', webSearchOn); // reflect the default-on state

boot();
