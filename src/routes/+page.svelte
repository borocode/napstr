<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { open } from '@tauri-apps/plugin-dialog';

  type View = 'Search' | 'Downloads' | 'Shared' | 'Profile' | 'Settings';
  type WindowResizeDirection = 'East' | 'North' | 'NorthEast' | 'NorthWest' | 'South' | 'SouthEast' | 'SouthWest' | 'West';
  type Result = {
    id: number;
    name: string;
    format: string;
    size: string;
    sources: number;
    speed: string;
    length: string;
    fileId: string;
    sourceDetails?: SourceDetail[];
    remote?: boolean;
    artist?: string;
    album?: string;
    license?: string;
    description?: string;
  };
  type SourceDetail = { pubkey: string; npub: string; displayName: string; relay: string; about: string; picture: string; eventId: string };
  type Transfer = {
    id: number;
    name: string;
    size: string;
    speed: string;
    progress: number;
    status: string;
  };

  const views: { label: View; icon: string }[] = [
    { label: 'Search', icon: '⌕' },
    { label: 'Downloads', icon: '⇩' },
    { label: 'Shared', icon: '▤' },
    { label: 'Profile', icon: '☺' },
    { label: 'Settings', icon: '⚙' }
  ];

  const demoResults: Result[] = [
    { id: 1, name: 'Free Culture Orchestra — Overture', format: 'MP3', size: '11 MB', sources: 12, speed: 'Tor', length: '5:03', fileId: '9BD2 186C F11A 702E…', remote: true },
    { id: 2, name: 'Open Tape Archive — Night Train', format: 'OGG', size: '8.4 MB', sources: 7, speed: 'Tor', length: '3:46', fileId: 'D4E7 B90A 3C18 8EE2…', remote: true },
    { id: 3, name: 'Copyleft Sessions — First Light', format: 'FLAC', size: '31 MB', sources: 4, speed: 'Tor', length: '4:12', fileId: 'A81C 0F42 76B9 94D1…', remote: true },
    { id: 4, name: 'Commons Choir — Homeward', format: 'OPUS', size: '6.1 MB', sources: 2, speed: 'Tor', length: '4:31', fileId: '72A0 ECF1 354C 82B7…', remote: true }
  ];

  type NativeFile = { fileId: string; filename: string; path: string; size: number; format: string; chunkCount: number; status: string; title: string; artist: string; album: string; mime: string; license: string; description: string; tags: string };
  type NativeTransfer = { id: number; fileId: string; filename: string; size: number; progress: number; status: string; speed: string; destination: string };
  type NativeSettings = { sharedFolder: string; downloadFolder: string; nostrRelays: string; displayName: string; profileAbout: string; profilePicture: string };
  type Snapshot = { files: NativeFile[]; transfers: NativeTransfer[]; settings: NativeSettings; indexedBytes: number; native: boolean };
  type NetworkStatus = { connected: boolean; npub: string; pubkey: string; relayCount: number; torRunning: boolean; error: string };
  type NetworkResult = { fileId: string; filename: string; title: string; artist: string; album: string; format: string; mime: string; size: number; license: string; description: string; tags: string; sources: SourceDetail[] };

  let activeView: View = 'Search';
  let results: Result[] = demoResults;
  let query = '';
  let format = 'Audio only';
  let minimumSources = 1;
  let maximumSize = '';
  let searchedQuery = 'All audio';
  let selected: Result | null = results[0];
  let advanced = false;
  let paused = false;
  let aboutOpen = false;
  let sourceProfile: SourceDetail | null = null;
  let clock = '';
  let nativeReady = false;
  let activityMessage = 'Browser preview — open in the desktop app to use local files';
  let sharedFolder = '';
  let downloadFolder = '';
  let nostrRelays = 'wss://relay.damus.io, wss://nos.lol, wss://relay.nostr.com, wss://relay.primal.net, wss://relay.snort.social, wss://nostr.mom';
  let displayName = 'napstr-user';
  let profileAbout = 'Sharing files privately with Napstr.';
  let profilePicture = '';
  let indexedBytes = 0;
  let networkConnected = false;
  let torRunning = false;
  let identityNpub = '';
  let networkError = '';
  let selectedSource = 0;
  let selectedShared: NativeFile | null = null;
  let transferPaneHeight = 119;
  let stopTransferResize = () => {};
  let transfers: Transfer[] = [
    { id: 1, name: 'Copyleft Sessions — First Light.flac', size: '31 MB', speed: '1.8 MB/s', progress: 74, status: 'Downloading verified audio' },
    { id: 2, name: 'Open Tape Archive — Night Train.ogg', size: '8.4 MB', speed: '620 KB/s', progress: 42, status: 'Downloading verified audio' }
  ];

  let sharedFiles: Array<NativeFile & { name: string; readableSize: string; peers: number }> = [
    { fileId: '', filename: 'My Copyleft Track.flac', name: 'My Copyleft Track.flac', path: '', size: 31 * 1024 ** 2, readableSize: '31 MB', format: 'FLAC', chunkCount: 0, status: 'Demo', peers: 0, title: '', artist: '', album: '', mime: 'audio/flac', license: 'copyleft', description: '', tags: '' },
    { fileId: '', filename: 'Commons Recording.ogg', name: 'Commons Recording.ogg', path: '', size: 8.4 * 1024 ** 2, readableSize: '8.4 MB', format: 'OGG', chunkCount: 0, status: 'Demo', peers: 0, title: '', artist: '', album: '', mime: 'audio/ogg', license: 'copyleft', description: '', tags: '' }
  ];

  const readableSize = (bytes: number) => {
    if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
    if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(bytes >= 100 * 1024 ** 2 ? 0 : 1)} MB`;
    if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
    return `${bytes} B`;
  };

  function mapFiles(files: NativeFile[]): Result[] {
    return files.map((file, index) => ({
      id: index + 1, name: file.filename, format: file.format, size: readableSize(file.size), sources: 1,
      speed: 'Local', length: '—', fileId: file.fileId, artist: file.artist, album: file.album, license: file.license, description: file.description
    }));
  }

  function mapNetworkFiles(files: NetworkResult[]): Result[] {
    const localFileIds = new Set(sharedFiles.map((file) => file.fileId));
    return files.map((file, index) => {
      const local = localFileIds.has(file.fileId);
      return {
        id: index + 1, name: file.title || file.filename, format: file.format, size: readableSize(file.size),
        sources: file.sources.length, speed: local ? 'Local' : 'Tor', length: '—', fileId: file.fileId,
        sourceDetails: file.sources, remote: !local, artist: file.artist, album: file.album,
        license: file.license, description: file.description
      };
    });
  }

  function matchesType(mime: string, fileFormat: string) {
    return mime.startsWith('audio/') && ['MP3', 'FLAC', 'WAV', 'OGG', 'OPUS'].includes(fileFormat.toUpperCase());
  }

  function maximumBytes() {
    const match = maximumSize.trim().match(/^(\d+(?:\.\d+)?)\s*(B|KB|MB|GB|TB)?$/i);
    if (!match) return Number.POSITIVE_INFINITY;
    const units: Record<string, number> = { B: 1, KB: 1024, MB: 1024 ** 2, GB: 1024 ** 3, TB: 1024 ** 4 };
    return Number(match[1]) * units[(match[2] || 'B').toUpperCase()];
  }

  function isActiveTransfer(transfer: Transfer) {
    return transfer.progress < 100 && !/^(Failed|Cancelled|Refused|All seeders refused)/.test(transfer.status);
  }

  function applySnapshot(snapshot: Snapshot) {
    nativeReady = snapshot.native;
    indexedBytes = snapshot.indexedBytes;
    sharedFolder = snapshot.settings.sharedFolder;
    downloadFolder = snapshot.settings.downloadFolder;
    nostrRelays = snapshot.settings.nostrRelays;
    displayName = snapshot.settings.displayName;
    profileAbout = snapshot.settings.profileAbout;
    profilePicture = snapshot.settings.profilePicture;
    sharedFiles = snapshot.files.map((file) => ({ ...file, name: file.filename, readableSize: readableSize(file.size), peers: 0 }));
    if (selectedShared) selectedShared = snapshot.files.find((file) => file.fileId === selectedShared?.fileId) ?? null;
    results = mapFiles(snapshot.files);
    selected = results[0] ?? null;
    searchedQuery = 'local catalogue';
    transfers = snapshot.transfers.map((transfer) => ({ id: transfer.id, name: transfer.filename, size: readableSize(transfer.size), speed: transfer.speed, progress: transfer.progress, status: transfer.status }));
    activityMessage = snapshot.files.length ? `${snapshot.files.length} local file(s) indexed and ready` : 'Choose a shared folder to begin';
  }

  async function refreshSnapshot() {
    try { applySnapshot(await invoke<Snapshot>('get_snapshot')); } catch { nativeReady = false; }
  }

  async function connectNetwork() {
    if (!nativeReady) return;
    activityMessage = 'Connecting to Nostr relays and opening encrypted inbox…';
    try {
      const status = await invoke<NetworkStatus>('start_network');
      networkConnected = status.connected;
      torRunning = status.torRunning;
      identityNpub = status.npub;
      networkError = status.error;
      activityMessage = `Nostr connected · loading the most available audio from ${status.relayCount} relay(s)…`;
      await search();
    } catch (error) {
      networkConnected = false;
      networkError = String(error);
      activityMessage = `Network unavailable: ${String(error)}`;
    }
  }

  async function search() {
    searchedQuery = query.trim() || 'All audio';
    if (networkConnected) {
      try {
        const matches = await invoke<NetworkResult[]>('network_search', { query: query.trim() });
        const ranked = matches
          .filter((item) => item.sources.length >= minimumSources && item.size <= maximumBytes() && matchesType(item.mime, item.format))
          .sort((left, right) => right.sources.length - left.sources.length || left.filename.localeCompare(right.filename));
        results = mapNetworkFiles(ranked);
        activityMessage = `${results.length} globally aggregated file ID(s), ranked by active seeders`;
      } catch (error) { activityMessage = `Global search failed: ${String(error)}`; }
    } else if (nativeReady) {
      try {
        const matches = await invoke<NativeFile[]>('search_catalog', { query: query.trim() });
        results = mapFiles(matches.filter((item) => minimumSources <= 1 && item.size <= maximumBytes() && matchesType(item.mime, item.format)));
        activityMessage = `${results.length} local match(es) found`;
      } catch (error) { activityMessage = `Search failed: ${String(error)}`; }
    }
    selected = results[0] ?? null;
    selectedSource = 0;
  }

  async function startDownload() {
    if (!selected) return;
    if (nativeReady && !selected.remote) {
      activityMessage = `${selected.name} is already on this computer · use Play`;
      return;
    }
    if (transfers.some((item) => item.name.startsWith(selected!.name))) return;
    if (nativeReady) {
      const sources = selected.sourceDetails ?? [];
      if (!sources.length) { activityMessage = 'No seeder is available for this file'; return; }
      activityMessage = `Sending encrypted NIP-17 requests to ${sources.length} seeder(s)…`;
      try {
        await invoke('request_network_download', { fileId: selected.fileId, sourcePubkeys: sources.map((source) => source.pubkey) });
        await refreshSnapshot();
        activeView = 'Downloads';
        activityMessage = 'Encrypted requests sent · waiting for temporary onion services';
      } catch (error) { activityMessage = `Request failed: ${String(error)}`; }
      return;
    }
    transfers = [
      ...transfers,
      { id: Date.now(), name: `${selected.name}.${selected.format.toLowerCase()}`, size: selected.size, speed: 'Negotiating…', progress: 2, status: 'Requesting private Tor session' }
    ];
    activeView = 'Downloads';
  }

  async function playSelectedAudio() {
    if (!nativeReady || !selected || selected.remote) return;
    try {
      await invoke('play_shared_audio', { fileId: selected.fileId });
      activityMessage = `Opened ${selected.name} in the system audio player`;
    } catch (error) { activityMessage = `Playback failed: ${String(error)}`; }
  }

  async function activateSelected() {
    if (nativeReady && selected && !selected.remote) await playSelectedAudio();
    else await startDownload();
  }

  async function playTransferAudio(id: number) {
    if (!nativeReady) return;
    try {
      await invoke('play_transfer_audio', { id });
      activityMessage = 'Opened verified audio in the system player';
    } catch (error) { activityMessage = `Playback failed: ${String(error)}`; }
  }

  async function blockSelectedFile() {
    if (!nativeReady || !selected?.remote || !window.confirm('Block this SHA-256 file ID? Every seeder offering the exact same bytes will be hidden.')) return;
    try {
      await invoke('block_file', { fileId: selected.fileId });
      activityMessage = 'File hash blocked locally';
      await search();
    } catch (error) { activityMessage = `Could not block file: ${String(error)}`; }
  }

  async function blockSelectedUser() {
    const source = selected?.sourceDetails?.[selectedSource];
    if (!nativeReady || !source || !window.confirm(`Block ${source.displayName}? Their catalogue entries and requests will be ignored.`)) return;
    try {
      await invoke('block_user', { pubkey: source.pubkey });
      activityMessage = 'Nostr publisher blocked locally';
      await search();
    } catch (error) { activityMessage = `Could not block publisher: ${String(error)}`; }
  }

  async function reportSelectedFile() {
    const source = selected?.sourceDetails?.[selectedSource];
    if (!nativeReady || !selected?.remote || !source) return;
    const reason = window.prompt('Reason for the public, signed NIP-56 report:');
    if (!reason?.trim()) return;
    try {
      await invoke('report_catalogue', { fileId: selected.fileId, sourcePubkey: source.pubkey, eventId: source.eventId, reportType: 'illegal', reason: reason.trim() });
      activityMessage = 'Signed NIP-56 report published';
    } catch (error) { activityMessage = `Report failed: ${String(error)}`; }
  }

  async function removeTransfer(id: number) {
    if (nativeReady) {
      try { await invoke('cancel_transfer', { id }); await invoke('remove_transfer', { id }); } catch (error) { activityMessage = `Could not remove transfer: ${String(error)}`; }
    }
    transfers = transfers.filter((transfer) => transfer.id !== id);
  }

  async function togglePause() {
    paused = !paused;
    if (nativeReady) {
      try { await invoke('set_downloads_paused', { paused }); activityMessage = paused ? 'All active downloads paused' : 'Downloads resumed'; }
      catch (error) { activityMessage = `Could not change download state: ${String(error)}`; }
    }
  }

  async function chooseSharedFolder() {
    if (!nativeReady) { activityMessage = 'Folder selection is available in the packaged desktop app'; return; }
    try {
      const selectedPath = await open({ directory: true, multiple: false, title: 'Choose the folder Napstr may share', defaultPath: sharedFolder || undefined });
      if (!selectedPath || Array.isArray(selectedPath)) return;
      activityMessage = 'Indexing files and calculating SHA-256 hashes…';
      const report = await invoke<{ fileCount: number; totalBytes: number; errors: string[] }>('set_shared_folder', { path: selectedPath });
      await refreshSnapshot();
      if (networkConnected) await invoke('publish_catalogue');
      activityMessage = `Indexed ${report.fileCount} file(s), ${readableSize(report.totalBytes)}${report.errors.length ? ` · ${report.errors.length} skipped` : ''}`;
    } catch (error) { activityMessage = `Folder selection failed: ${String(error)}`; }
  }

  async function chooseDownloadFolder() {
    if (!nativeReady) { activityMessage = 'Folder selection is available in the packaged desktop app'; return; }
    try {
      const selectedPath = await open({ directory: true, multiple: false, title: 'Choose where Napstr saves downloads', defaultPath: downloadFolder || undefined });
      if (!selectedPath || Array.isArray(selectedPath)) return;
      downloadFolder = selectedPath;
      await persistSettings();
    } catch (error) { activityMessage = `Folder selection failed: ${String(error)}`; }
  }

  async function openDownloadsFolder() {
    if (!nativeReady) return;
    try { await invoke('open_downloads_folder'); }
    catch (error) { activityMessage = `Could not open downloads folder: ${String(error)}`; }
  }

  async function rescanSharedFolder() {
    if (!nativeReady) return;
    activityMessage = 'Rescanning shared folder…';
    try {
      const report = await invoke<{ fileCount: number; totalBytes: number }>('rescan_shared_folder');
      await refreshSnapshot();
      if (networkConnected) await invoke('publish_catalogue');
      activityMessage = `Indexed ${report.fileCount} file(s), ${readableSize(report.totalBytes)}`;
    } catch (error) { activityMessage = `Rescan failed: ${String(error)}`; }
  }

  async function persistSettings() {
    if (!nativeReady) return;
    try {
      applySnapshot(await invoke<Snapshot>('save_settings', { settings: { sharedFolder, downloadFolder, nostrRelays, displayName, profileAbout, profilePicture } }));
      if (networkConnected) await invoke('publish_profile');
      activityMessage = networkConnected ? 'Settings saved and profile published' : 'Settings saved';
    } catch (error) { activityMessage = `Could not save settings: ${String(error)}`; }
  }

  async function saveFileMetadata() {
    if (!nativeReady || !selectedShared) return;
    try {
      selectedShared = await invoke<NativeFile>('update_file_metadata', { metadata: selectedShared });
      await refreshSnapshot();
      if (networkConnected) await invoke('publish_catalogue');
      activityMessage = 'Metadata saved and catalogue republished';
    } catch (error) { activityMessage = `Metadata update failed: ${String(error)}`; }
  }

  const windowCommand = async (command: 'minimise_window' | 'toggle_maximise' | 'close_window') => {
    if (nativeReady) await invoke(command);
  };

  function beginWindowResize(event: PointerEvent, direction: WindowResizeDirection) {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    getCurrentWindow().startResizeDragging(direction).catch(() => {});
  }

  function transferPaneMaximum() {
    return typeof window === 'undefined' ? 300 : Math.max(80, window.innerHeight - 340);
  }

  function setTransferPaneHeight(height: number, remember = false) {
    transferPaneHeight = Math.round(Math.min(transferPaneMaximum(), Math.max(48, height)));
    if (remember) window.localStorage.setItem('napstr-transfer-pane-height', String(transferPaneHeight));
  }

  function beginTransferResize(event: PointerEvent) {
    if (event.button !== 0) return;
    event.preventDefault();
    stopTransferResize();
    const startY = event.clientY;
    const startHeight = transferPaneHeight;
    const move = (next: PointerEvent) => setTransferPaneHeight(startHeight + startY - next.clientY);
    const stop = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', stop);
      window.removeEventListener('pointercancel', stop);
      document.body.classList.remove('resizing-transfer-pane');
      window.localStorage.setItem('napstr-transfer-pane-height', String(transferPaneHeight));
      stopTransferResize = () => {};
    };
    stopTransferResize = stop;
    document.body.classList.add('resizing-transfer-pane');
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', stop);
    window.addEventListener('pointercancel', stop);
  }

  function resizeTransferWithKeyboard(event: KeyboardEvent) {
    if (event.key === 'ArrowUp') setTransferPaneHeight(transferPaneHeight + 20, true);
    else if (event.key === 'ArrowDown') setTransferPaneHeight(transferPaneHeight - 20, true);
    else if (event.key === 'Home') setTransferPaneHeight(48, true);
    else if (event.key === 'End') setTransferPaneHeight(transferPaneMaximum(), true);
    else return;
    event.preventDefault();
  }

  onMount(() => {
    const savedTransferHeight = Number(window.localStorage.getItem('napstr-transfer-pane-height'));
    setTransferPaneHeight(Number.isFinite(savedTransferHeight) && savedTransferHeight > 0 ? savedTransferHeight : window.innerHeight < 700 ? 94 : 119);
    const clampTransferPane = () => setTransferPaneHeight(transferPaneHeight);
    window.addEventListener('resize', clampTransferPane);
    refreshSnapshot().then(connectNetwork);
    const updateClock = () => {
      clock = new Intl.DateTimeFormat('en-GB', { hour: '2-digit', minute: '2-digit' }).format(new Date());
    };
    updateClock();
    const clockTimer = window.setInterval(updateClock, 30000);
    const networkTimer = window.setInterval(() => {
      if (!nativeReady) return;
      invoke<NetworkStatus>('network_status').then((status) => {
        networkConnected = status.connected; torRunning = status.torRunning; identityNpub = status.npub; networkError = status.error;
      }).catch(() => {});
    }, 5000);
    const transferTimer = window.setInterval(() => {
      if (nativeReady) {
        invoke<NativeTransfer[]>('get_transfers').then((items) => {
          transfers = items.map((transfer) => ({ id: transfer.id, name: transfer.filename, size: readableSize(transfer.size), speed: transfer.speed, progress: transfer.progress, status: transfer.status }));
        }).catch(() => {});
        return;
      }
      if (paused) return;
      transfers = transfers.map((item) => {
        if (item.progress >= 100) return item;
        const progress = Math.min(100, item.progress + 0.4);
        return {
          ...item,
          progress,
          speed: item.progress < 4 ? 'Connecting…' : item.speed === 'Negotiating…' ? '892 KB/s' : item.speed,
          status: progress === 100 ? 'Verified · Complete' : item.progress < 4 ? 'Opening onion service' : item.status
        };
      });
    }, 1000);
    return () => {
      clearInterval(clockTimer);
      clearInterval(networkTimer);
      clearInterval(transferTimer);
      window.removeEventListener('resize', clampTransferPane);
      stopTransferResize();
    };
  });
</script>

<svelte:head><title>Napstr — Nostr file sharing</title></svelte:head>

<main class="desktop">
  <section class="app-window" style={`--transfer-height: ${transferPaneHeight}px`} aria-label="Napstr application window">
    <button class="window-resize-handle resize-n" aria-label="Resize window from top" onpointerdown={(event) => beginWindowResize(event, 'North')}></button>
    <button class="window-resize-handle resize-e" aria-label="Resize window from right" onpointerdown={(event) => beginWindowResize(event, 'East')}></button>
    <button class="window-resize-handle resize-s" aria-label="Resize window from bottom" onpointerdown={(event) => beginWindowResize(event, 'South')}></button>
    <button class="window-resize-handle resize-w" aria-label="Resize window from left" onpointerdown={(event) => beginWindowResize(event, 'West')}></button>
    <button class="window-resize-handle resize-ne" aria-label="Resize window from top right" onpointerdown={(event) => beginWindowResize(event, 'NorthEast')}></button>
    <button class="window-resize-handle resize-se" aria-label="Resize window from bottom right" onpointerdown={(event) => beginWindowResize(event, 'SouthEast')}></button>
    <button class="window-resize-handle resize-sw" aria-label="Resize window from bottom left" onpointerdown={(event) => beginWindowResize(event, 'SouthWest')}></button>
    <button class="window-resize-handle resize-nw" aria-label="Resize window from top left" onpointerdown={(event) => beginWindowResize(event, 'NorthWest')}></button>

    <header class="titlebar" data-tauri-drag-region>
      <div class="title-left"><span class="app-icon"><img src="/napstr-logo.png" alt="" /></span><span>Napstr</span></div>
      <div class="window-controls" aria-hidden="true">
        <button tabindex="-1" onclick={() => windowCommand('minimise_window')}>_</button><button tabindex="-1" onclick={() => windowCommand('toggle_maximise')}>□</button><button tabindex="-1" onclick={() => windowCommand('close_window')}>×</button>
      </div>
    </header>

    <div class="toolbar">
      <div class="toolbar-brand" title="Napstr home">
        <img src="/napstr-logo.png" alt="Napstr" />
      </div>
      <div class="toolbar-separator"></div>
      {#each views as view}
        <button class:active={activeView === view.label} class="tool-button" onclick={() => (activeView = view.label)}>
          <span class="tool-icon icon-{view.label.toLowerCase()}">{view.icon}</span>
          <span>{view.label}</span>
        </button>
      {/each}
      <div class="toolbar-spacer"></div>
      <button class="connection-box" onclick={connectNetwork} title={networkError || 'Reconnect network'}><span class:amber={!networkConnected} class="led"></span><strong>{networkConnected ? 'Nostr connected' : nativeReady ? 'Connect network' : 'Preview mode'}</strong><small>{networkConnected ? `${torRunning ? 'Tor active' : 'Tor on demand'} · NIP-17 ready` : nativeReady ? 'Click to retry' : 'Native features unavailable'}</small></button>
      <button class="tool-button help-button" onclick={() => (aboutOpen = true)}><span class="tool-icon">?</span><span>About</span></button>
    </div>

    <div class="network-strip">
      <span class="network-pulse">▥</span>
      <span>{activityMessage}</span>
      <span class="strip-right">{nativeReady ? displayName : 'demo@napstr'} <i class:amber={!nativeReady} class="led"></i></span>
    </div>

    <div class="workspace">
      {#if activeView === 'Search'}
        <section class="panel search-panel">
          <div class="panel-title"><span></span><b>Search the Napstr network</b><span></span></div>
          <form class="search-form" onsubmit={(e) => { e.preventDefault(); search(); }}>
            <label for="search-query">Search:</label>
            <input id="search-query" bind:value={query} />
            <label for="format">File type:</label>
            <select id="format" bind:value={format} disabled><option>Audio only</option></select>
            <button class="classic-button primary" type="submit">Search</button>
          </form>
          <button class="advanced-toggle" onclick={() => (advanced = !advanced)}><span>{advanced ? '▼' : '▶'}</span> {advanced ? 'Hide' : 'Show'} advanced search options</button>
          {#if advanced}
            <div class="advanced-row"><label>Minimum seeders: <input type="number" bind:value={minimumSources} min="1" /></label><label>Maximum size: <input bind:value={maximumSize} placeholder="e.g. 2 GB" /></label><label><input type="checkbox" checked disabled /> Online seeders only</label></div>
          {/if}
        </section>

        <div class="split-content">
          <section class="results-pane" aria-label="Search results">
            <div class="section-caption"><span>Search results for “{searchedQuery}”</span><small>{results.length} file IDs found</small></div>
            <div class="table-wrap">
              <table class="file-table">
                <thead><tr><th class="name-col">Name</th><th>Type</th><th class="number">Size</th><th class="number">Seeders</th><th>Line speed</th><th>Length</th></tr></thead>
                <tbody>
                  {#each results as item}
                    <tr class:selected={selected?.id === item.id} onclick={() => (selected = item)} ondblclick={activateSelected}>
                      <td><span class="file-icon">▶</span>{item.name}</td><td>{item.format}</td><td class="number">{item.size}</td><td class="number"><span class="source-dot"></span>{item.sources}</td><td>{item.speed}</td><td>{item.length}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
            <div class="scrollbar horizontal"><button>◀</button><div><span style="width: 66%"></span></div><button>▶</button></div>
          </section>

          <aside class="details-pane">
            <div class="section-caption"><span>File details</span></div>
            {#if selected}
              <div class="selected-file">
                <div class="large-file-icon">▶</div>
                <div><strong>{selected.name}</strong><span>{selected.format} · {selected.size} · {selected.length}</span><small>File ID: {selected.fileId}</small></div>
              </div>
              {#if selected.artist || selected.album || selected.description}<div class="file-metadata"><b>{selected.artist || 'Unknown creator'}</b>{#if selected.album}<span> · {selected.album}</span>{/if}{#if selected.description}<p>{selected.description}</p>{/if}{#if selected.license}<small>License: {selected.license}</small>{/if}</div>{/if}
              <fieldset><legend>Seeders</legend>
                <div class="sources-list">
                  {#if selected.remote}
                    {#each selected.sourceDetails ?? [] as source, index}
                      <button class:selected-source={selectedSource === index} class="source-row" onclick={() => (selectedSource = index)}><span class="user-icon">☺</span><b>{source.displayName}</b><small>{source.npub.slice(0, 12)}…</small><span class="online"><i></i> Seeding</span></button>
                    {/each}
                  {:else}
                    <div><span class="user-icon">☺</span><b>This computer</b><small>Local</small><span class="online"><i></i> Ready</span></div>
                  {/if}
                </div>
              </fieldset>
              <div class="detail-actions">{#if selected.remote}<button class="classic-button primary" onclick={startDownload}>⇩ Download</button><button class="classic-button" onclick={() => (sourceProfile = selected?.sourceDetails?.[selectedSource] ?? null)}>View profile</button>{:else}<button class="classic-button primary" onclick={playSelectedAudio}>▶ Play</button>{/if}</div>
              {#if selected.remote}<div class="detail-actions moderation-actions"><button class="classic-button" onclick={reportSelectedFile}>Report</button><button class="classic-button" onclick={blockSelectedFile}>Block file</button><button class="classic-button" onclick={blockSelectedUser}>Block user</button></div>{/if}
              {#if selected.remote}<p class="privacy-note"><span>♜</span> Transfer will use a private, temporary Tor onion service.</p>{:else}<p class="privacy-note"><span>♬</span> This audio is already stored locally and will not be downloaded again.</p>{/if}
            {:else}<p class="empty-state">Select a result to see active seeders.</p>{/if}
          </aside>
        </div>
      {:else if activeView === 'Downloads'}
        <section class="full-panel">
          <div class="panel-title"><span></span><b>Download Manager</b><span></span></div>
          <div class="actionbar"><button class="classic-button" onclick={togglePause}>{paused ? '▶ Resume all' : 'Ⅱ Pause all'}</button><button class="classic-button" onclick={openDownloadsFolder}>Open downloads folder</button><div class="spacer"></div><span>{transfers.filter(isActiveTransfer).length} active transfer(s)</span></div>
          <table class="file-table download-table"><thead><tr><th>Download order</th><th>Progress</th><th>Size</th><th>Speed</th><th>Status</th><th></th></tr></thead><tbody>
            {#each transfers as transfer}
              <tr><td><span class="download-arrow">⇩</span>{transfer.name}</td><td><div class="progress"><span style={`width:${transfer.progress}%`}></span><b>{Math.round(transfer.progress)}%</b></div></td><td>{transfer.size}</td><td>{transfer.speed}</td><td>{transfer.status}</td><td>{#if transfer.status === 'Verified · Complete'}<button class="tiny-button" onclick={() => playTransferAudio(transfer.id)} title="Play verified audio">▶</button>{/if}<button class="tiny-button" onclick={() => removeTransfer(transfer.id)} title="Remove">×</button></td></tr>
            {/each}
          </tbody></table>
          {#if transfers.length === 0}<p class="empty-state">There are no downloads in the queue.</p>{/if}
        </section>
      {:else if activeView === 'Shared'}
        <section class="full-panel">
          <div class="panel-title"><span></span><b>My Shared Files</b><span></span></div>
          <div class="actionbar"><button class="classic-button primary" onclick={chooseSharedFolder}>＋ Add shared folder</button><button class="classic-button" onclick={rescanSharedFolder}>↻ Rescan</button><div class="spacer"></div><span>Sharing {sharedFiles.length} files · {readableSize(indexedBytes)}</span></div>
          <div class="folder-path"><b>Shared folder:</b><input value={sharedFolder || 'No folder selected'} readonly /><button class="classic-button" onclick={chooseSharedFolder}>Browse…</button></div>
          <table class="file-table"><thead><tr><th>Name</th><th>Size</th><th>Catalogue</th><th>Active peers</th></tr></thead><tbody>{#each sharedFiles as file}<tr class:selected={selectedShared?.fileId === file.fileId} onclick={() => (selectedShared = { ...file })}><td><span class="file-icon">▶</span>{file.name}</td><td>{file.readableSize}</td><td><span class:amber={!networkConnected} class="led"></span>{networkConnected ? 'Published' : 'Indexed'}</td><td>{file.peers}</td></tr>{/each}</tbody></table>
          {#if selectedShared}
            <fieldset class="metadata-editor"><legend>Public catalogue metadata</legend>
              <label>Title <input bind:value={selectedShared.title} placeholder={selectedShared.filename} /></label>
              <label>Artist <input bind:value={selectedShared.artist} /></label>
              <label>Album <input bind:value={selectedShared.album} /></label>
              <label>MIME type <input value={selectedShared.mime} readonly /></label>
              <label>License <input bind:value={selectedShared.license} /></label>
              <label>Tags <input bind:value={selectedShared.tags} placeholder="comma,separated,tags" /></label>
              <label>Description <input bind:value={selectedShared.description} /></label>
              <button class="classic-button primary" onclick={saveFileMetadata}>Save &amp; publish</button>
            </fieldset>
          {/if}
          <p class="privacy-note wide"><span>♜</span> Only validated MP3, FLAC, WAV, Ogg Vorbis, and Opus audio is indexed recursively. Embedded cover artwork is allowed.</p>
        </section>
      {:else if activeView === 'Profile'}
        <section class="full-panel profile-view">
          <div class="panel-title"><span></span><b>Napstr Profile</b><span></span></div>
          <div class="profile-card"><div class="avatar"><img src="/napstr-logo.png" alt="Napstr mascot" /></div><div><h2>{displayName}</h2><p>Your dedicated Napstr Nostr identity.</p><code>{identityNpub || 'Connect to create identity'}</code><div class="profile-stats"><span><b>{sharedFiles.length}</b> shared files</span><span><b>{transfers.length}</b> transfers</span><span><b>{networkConnected ? 'Nostr online' : 'Offline'}</b></span></div></div></div>
          <fieldset class="edit-profile"><legend>Profile</legend><label>Display name <input bind:value={displayName} /></label><label>About <input bind:value={profileAbout} /></label><label>Picture URL <input bind:value={profilePicture} placeholder="https://…" /></label><button class="classic-button primary" onclick={persistSettings}>Save profile</button></fieldset>
          <p class="privacy-note wide"><span>i</span> Your profile and shared catalogue are public on Nostr. Transfer addresses and credentials are never published.</p>
        </section>
      {:else}
        <section class="full-panel settings-view">
          <div class="panel-title"><span></span><b>Napstr Settings</b><span></span></div>
          <fieldset><legend>Network</legend><label><input type="checkbox" checked disabled /> Connect automatically at startup</label><label><input type="checkbox" checked disabled /> Never allow direct-IP file transfer</label><label>Nostr relays <input bind:value={nostrRelays} /></label><label>Tor <input value="Bundled, managed automatically" readonly /></label></fieldset>
          <fieldset><legend>Downloads</legend><label>Save files to <input bind:value={downloadFolder} /><button class="classic-button" onclick={chooseDownloadFolder}>Browse…</button></label><label>Chunk size <select disabled><option>1 MB</option></select></label><label><input type="checkbox" checked disabled /> Verify every chunk and final SHA-256</label></fieldset>
          <div class="settings-actions"><button class="classic-button primary" onclick={persistSettings}>OK</button><button class="classic-button" onclick={refreshSnapshot}>Cancel</button><button class="classic-button" onclick={persistSettings}>Apply</button></div>
        </section>
      {/if}
    </div>

    <section class="transfer-dock">
      <button
        type="button"
        class="dock-resizer"
        aria-label="Resize Transfer Manager"
        title="Drag to resize Transfer Manager · double-click to reset"
        onpointerdown={beginTransferResize}
        onkeydown={resizeTransferWithKeyboard}
        ondblclick={() => setTransferPaneHeight(window.innerHeight < 700 ? 94 : 119, true)}
      ></button>
      <div class="dock-title"><span></span><b>Transfer Manager</b><span></span><button onclick={() => (activeView = 'Downloads')}>□</button></div>
      <div class="mini-transfers">
        {#each transfers.slice(0, 2) as transfer}
          <div class="mini-row"><span class="download-arrow">⇩</span><span class="mini-name">{transfer.name}</span><div class="progress"><span style={`width:${transfer.progress}%`}></span></div><span>{transfer.size}</span><span>{transfer.speed}</span></div>
        {/each}
      </div>
    </section>

    <footer class="statusbar"><span>{activityMessage}</span><span><i class:amber={!networkConnected} class="led"></i> Nostr {networkConnected ? 'online' : 'offline'}</span><span>♜ Tor: {torRunning ? 'running' : 'starts on demand'}</span><span class="status-clock">{clock}</span></footer>
  </section>

  {#if aboutOpen}
    <div class="modal-backdrop" role="presentation" onclick={() => (aboutOpen = false)}>
      <dialog class="dialog" open aria-label="About Napstr" onclick={(e) => e.stopPropagation()} onkeydown={(e) => { if (e.key === 'Escape') aboutOpen = false; }}>
        <header class="titlebar"><div class="title-left"><span class="app-icon"><img src="/napstr-logo.png" alt="" /></span><span>About Napstr</span></div><div class="window-controls"><button onclick={() => (aboutOpen = false)}>×</button></div></header>
        <div class="dialog-body"><div class="about-logo"><img src="/napstr-logo.png" alt="" /></div><div><h2>Napstr</h2><p>Version 0.1.0</p><p>Public discovery over Nostr.<br />Private verified transfers over Tor.</p></div></div>
        <div class="dialog-actions"><button class="classic-button primary" onclick={() => (aboutOpen = false)}>OK</button></div>
      </dialog>
    </div>
  {/if}

  {#if sourceProfile}
    <div class="modal-backdrop" role="presentation" onclick={() => (sourceProfile = null)}>
      <dialog class="dialog" open aria-label="Napstr public profile" onclick={(e) => e.stopPropagation()}>
        <header class="titlebar"><div class="title-left"><span class="app-icon"><img src="/napstr-logo.png" alt="" /></span><span>Public Napstr Profile</span></div><div class="window-controls"><button onclick={() => (sourceProfile = null)}>×</button></div></header>
        <div class="dialog-body"><div class="about-logo">☺</div><div><h2>{sourceProfile.displayName}</h2><p>{sourceProfile.about || 'No profile description published.'}</p><code>{sourceProfile.npub}</code></div></div>
        <div class="dialog-actions"><button class="classic-button primary" onclick={() => (sourceProfile = null)}>OK</button></div>
      </dialog>
    </div>
  {/if}
</main>
