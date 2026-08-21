<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { open } from '@tauri-apps/plugin-dialog';

  type View = 'Search' | 'Downloads' | 'Shared' | 'Profile' | 'Settings';
  type PlayerMode = 'single' | 'folder' | 'all';
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
    fileId: string;
    name: string;
    size: string;
    speed: string;
    progress: number;
    status: string;
    destination: string;
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

  type NativeFile = { fileId: string; filename: string; path: string; folder: string; size: number; format: string; status: string; title: string; artist: string; album: string; mime: string; license: string; description: string; tags: string };
  type NativeTransfer = { id: number; fileId: string; filename: string; size: number; progress: number; status: string; speed: string; destination: string };
  type NativeSettings = { napstrFolder: string; nostrRelays: string; displayName: string; profileAbout: string; profilePicture: string };
  type Snapshot = { files: NativeFile[]; transfers: NativeTransfer[]; settings: NativeSettings; indexedBytes: number; native: boolean };
  type NetworkStatus = { connected: boolean; npub: string; pubkey: string; relayCount: number; torRunning: boolean; torStarting: boolean; torProgress: number; torError: string; error: string };
  type NetworkResult = { fileId: string; filename: string; title: string; artist: string; album: string; format: string; mime: string; size: number; license: string; description: string; tags: string; sources: SourceDetail[] };
  type PlayerTrack = { fileId: string; name: string; folder: string; artist: string; mime: string };
  type PlaybackStatus = { fileId: string; currentTime: number; duration: number; playing: boolean; ended: boolean };
  type BlockConfirmation =
    | { kind: 'file'; fileId: string; label: string }
    | { kind: 'user'; pubkey: string; label: string };

  let activeView: View = 'Search';
  let results: Result[] = demoResults;
  let query = '';
  let format = 'Audio only';
  let minimumSources = 1;
  let maximumSize = '';
  let searchedQuery = 'All audio';
  let resultsAreNetwork = false;
  let selected: Result | null = results[0];
  let advanced = false;
  let paused = false;
  let aboutOpen = false;
  let sourceProfile: SourceDetail | null = null;
  let blockConfirmation: BlockConfirmation | null = null;
  let blockInProgress = false;
  let startingDownloads = new Set<string>();
  let clock = '';
  let nativeReady = false;
  let activityMessage = 'Browser preview — open in the desktop app to use local files';
  let napstrFolder = '';
  let nostrRelays = 'wss://relay.damus.io, wss://nos.lol, wss://relay.nostr.com, wss://relay.primal.net, wss://relay.snort.social, wss://nostr.mom';
  let displayName = 'napstr-user';
  let profileAbout = 'Sharing files privately with Napstr. napstr.net';
  let profilePicture = '';
  let indexedBytes = 0;
  let networkConnected = false;
  let torRunning = false;
  let torStarting = false;
  let torProgress = 0;
  let torError = '';
  let identityNpub = '';
  let networkError = '';
  let selectedSource = 0;
  let selectedShared: NativeFile | null = null;
  let libraryFolderView = '*';
  let playerMode: PlayerMode = 'single';
  let playerQueue: PlayerTrack[] = [];
  let playerQueueIndex = -1;
  let currentTrack: PlayerTrack | null = null;
  let playerPlaying = false;
  let playerLoading = false;
  let playerCurrentTime = 0;
  let playerDuration = 0;
  let playerVolume = 0.85;
  let playerEnded = false;
  let transferPaneHeight = 119;
  let stopTransferResize = () => {};
  let transfers: Transfer[] = [
    { id: 1, fileId: '', name: 'Copyleft Sessions — First Light.flac', size: '31 MB', speed: '1.8 MB/s', progress: 74, status: 'Downloading verified audio', destination: '' },
    { id: 2, fileId: '', name: 'Open Tape Archive — Night Train.ogg', size: '8.4 MB', speed: '620 KB/s', progress: 42, status: 'Downloading verified audio', destination: '' }
  ];

  let sharedFiles: Array<NativeFile & { name: string; readableSize: string; peers: number }> = [
    { fileId: '', filename: 'My Copyleft Track.flac', name: 'My Copyleft Track.flac', path: '', folder: 'Albums/Example', size: 31 * 1024 ** 2, readableSize: '31 MB', format: 'FLAC', status: 'Demo', peers: 0, title: '', artist: '', album: '', mime: 'audio/flac', license: 'copyleft', description: '', tags: '' },
    { fileId: '', filename: 'Commons Recording.ogg', name: 'Commons Recording.ogg', path: '', folder: '', size: 8.4 * 1024 ** 2, readableSize: '8.4 MB', format: 'OGG', status: 'Demo', peers: 0, title: '', artist: '', album: '', mime: 'audio/ogg', license: 'copyleft', description: '', tags: '' }
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

  function isCompleteTransfer(transfer: Transfer) {
    return transfer.progress >= 100 && transfer.status === 'Verified · Complete' && Boolean(transfer.destination);
  }

  function isFinishedTransfer(transfer: Transfer) {
    return isCompleteTransfer(transfer) || /^(Failed|Cancelled|Refused|All seeders refused)/.test(transfer.status);
  }

  function mapTransfers(items: NativeTransfer[]): Transfer[] {
    return items.map((transfer) => ({
      id: transfer.id,
      fileId: transfer.fileId,
      name: transfer.filename,
      size: readableSize(transfer.size),
      speed: transfer.speed,
      progress: transfer.progress,
      status: transfer.status,
      destination: transfer.destination
    }));
  }

  function isLocalFile(fileId: string) {
    return sharedFiles.some((file) => file.fileId === fileId);
  }

  function folderName(folder: string) {
    return folder || '(Napstr folder)';
  }

  function libraryFolders() {
    return [...new Set(sharedFiles.map((file) => file.folder))]
      .sort((left, right) => folderName(left).localeCompare(folderName(right)));
  }

  function visibleSharedFiles() {
    return libraryFolderView === '*'
      ? sharedFiles
      : sharedFiles.filter((file) => file.folder === libraryFolderView);
  }

  function toPlayerTrack(file: NativeFile): PlayerTrack {
    return {
      fileId: file.fileId,
      name: file.title || file.filename,
      folder: file.folder,
      artist: file.artist,
      mime: file.mime
    };
  }

  function sortedLibraryTracks() {
    return sharedFiles
      .map(toPlayerTrack)
      .sort((left, right) => left.folder.localeCompare(right.folder) || left.name.localeCompare(right.name));
  }

  function queueForTrack(track: PlayerTrack, mode: PlayerMode) {
    const library = sortedLibraryTracks();
    if (!library.some((item) => item.fileId === track.fileId)) return [track];
    if (mode === 'all') return library;
    if (mode === 'folder') return library.filter((item) => item.folder === track.folder);
    return [track];
  }

  function formatPlayerTime(seconds: number) {
    if (!Number.isFinite(seconds) || seconds < 0) return '0:00';
    const whole = Math.floor(seconds);
    return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, '0')}`;
  }

  async function loadPlayerTrack(index: number) {
    const track = playerQueue[index];
    if (!track || playerLoading) return;
    playerLoading = true;
    playerQueueIndex = index;
    currentTrack = track;
    playerCurrentTime = 0;
    playerDuration = 0;
    playerEnded = false;
    try {
      applyPlaybackStatus(await invoke<PlaybackStatus>('play_audio', { fileId: track.fileId, volume: playerVolume }));
      activityMessage = `Playing ${track.name}${track.folder ? ` · ${track.folder}` : ''}`;
    } catch (error) {
      playerPlaying = false;
      activityMessage = `Playback failed: ${String(error)}`;
    } finally {
      playerLoading = false;
    }
  }

  async function playAudio(fileId: string, name: string, mode: PlayerMode = playerMode) {
    if (!nativeReady || !fileId) return;
    const indexed = sharedFiles.find((file) => file.fileId === fileId);
    const track = indexed
      ? toPlayerTrack(indexed)
      : { fileId, name, folder: '', artist: '', mime: '' };
    playerMode = indexed ? mode : 'single';
    playerQueue = queueForTrack(track, playerMode);
    const index = Math.max(0, playerQueue.findIndex((item) => item.fileId === fileId));
    await loadPlayerTrack(index);
  }

  async function togglePlayer() {
    if (!currentTrack) {
      if (selectedShared) await playAudio(selectedShared.fileId, selectedShared.filename);
      else if (selected && isLocalFile(selected.fileId)) await playAudio(selected.fileId, selected.name);
      else activityMessage = 'Select a local song to play';
      return;
    }
    if (playerEnded) {
      await loadPlayerTrack(playerQueueIndex);
      return;
    }
    try { applyPlaybackStatus(await invoke<PlaybackStatus>('toggle_audio')); }
    catch (error) { activityMessage = `Playback failed: ${String(error)}`; }
  }

  async function stopPlayer() {
    try { applyPlaybackStatus(await invoke<PlaybackStatus>('stop_audio')); }
    catch (error) { activityMessage = `Could not stop playback: ${String(error)}`; return; }
    playerEnded = false;
    if (currentTrack) activityMessage = `Stopped ${currentTrack.name}`;
  }

  async function nextPlayerTrack() {
    if (playerQueueIndex + 1 < playerQueue.length) await loadPlayerTrack(playerQueueIndex + 1);
    else stopPlayer();
  }

  async function previousPlayerTrack() {
    if (playerCurrentTime > 3 || playerQueueIndex <= 0) {
      try { applyPlaybackStatus(await invoke<PlaybackStatus>('seek_audio', { seconds: 0 })); }
      catch (error) { activityMessage = `Could not rewind playback: ${String(error)}`; }
      return;
    }
    await loadPlayerTrack(playerQueueIndex - 1);
  }

  async function playerTrackEnded() {
    playerPlaying = false;
    playerEnded = true;
    if (playerMode !== 'single' && playerQueueIndex + 1 < playerQueue.length) {
      await loadPlayerTrack(playerQueueIndex + 1);
    }
  }

  function changePlayerMode() {
    window.localStorage.setItem('napstr-player-mode', playerMode);
    if (!currentTrack) return;
    playerQueue = queueForTrack(currentTrack, playerMode);
    playerQueueIndex = Math.max(0, playerQueue.findIndex((item) => item.fileId === currentTrack?.fileId));
  }

  async function seekPlayer(event: Event) {
    try {
      applyPlaybackStatus(await invoke<PlaybackStatus>('seek_audio', { seconds: Number((event.currentTarget as HTMLInputElement).value) }));
      playerEnded = false;
    } catch (error) { activityMessage = `Could not seek in this track: ${String(error)}`; }
  }

  function changePlayerVolume(event: Event) {
    playerVolume = Number((event.currentTarget as HTMLInputElement).value);
    if (currentTrack) invoke<PlaybackStatus>('set_audio_volume', { volume: playerVolume }).catch(() => {});
    window.localStorage.setItem('napstr-player-volume', String(playerVolume));
  }

  function applyPlaybackStatus(status: PlaybackStatus) {
    if (!currentTrack || status.fileId !== currentTrack.fileId) return;
    playerCurrentTime = status.currentTime;
    playerDuration = status.duration;
    playerPlaying = status.playing;
  }

  function syncResultLocality() {
    const selectedFileId = selected?.fileId;
    if (resultsAreNetwork) {
      results = results.map((result) => {
        const local = isLocalFile(result.fileId);
        return { ...result, remote: !local, speed: local ? 'Local' : 'Tor' };
      });
    } else if (!query.trim() || searchedQuery === 'local catalogue' || searchedQuery === 'All audio') {
      results = mapFiles(sharedFiles);
    } else {
      results = results.filter((result) => isLocalFile(result.fileId));
    }
    selected = (selectedFileId ? results.find((result) => result.fileId === selectedFileId) : null) ?? results[0] ?? null;
  }

  function applySnapshot(snapshot: Snapshot) {
    nativeReady = snapshot.native;
    indexedBytes = snapshot.indexedBytes;
    napstrFolder = snapshot.settings.napstrFolder;
    nostrRelays = snapshot.settings.nostrRelays;
    displayName = snapshot.settings.displayName;
    profileAbout = snapshot.settings.profileAbout;
    profilePicture = snapshot.settings.profilePicture;
    sharedFiles = snapshot.files.map((file) => ({ ...file, name: file.filename, readableSize: readableSize(file.size), peers: 0 }));
    if (selectedShared) selectedShared = snapshot.files.find((file) => file.fileId === selectedShared?.fileId) ?? null;
    results = mapFiles(snapshot.files);
    resultsAreNetwork = false;
    selected = results[0] ?? null;
    searchedQuery = 'local catalogue';
    transfers = mapTransfers(snapshot.transfers);
    activityMessage = snapshot.files.length ? `${snapshot.files.length} local file(s) indexed and ready` : 'Choose a Napstr folder to begin';
  }

  async function refreshSnapshot() {
    try { applySnapshot(await invoke<Snapshot>('get_snapshot')); } catch { nativeReady = false; }
  }

  async function refreshLocalLibrary() {
    try {
      const snapshot = await invoke<Snapshot>('get_snapshot');
      indexedBytes = snapshot.indexedBytes;
      const nextFiles = snapshot.files.map((file) => ({ ...file, name: file.filename, readableSize: readableSize(file.size), peers: 0 }));
      const removedCurrentTrack = currentTrack && !nextFiles.some((file) => file.fileId === currentTrack?.fileId);
      sharedFiles = nextFiles;
      if (selectedShared) selectedShared = nextFiles.find((file) => file.fileId === selectedShared?.fileId) ?? null;
      if (removedCurrentTrack) {
        invoke<PlaybackStatus>('stop_audio').catch(() => {});
        currentTrack = null;
        playerQueue = [];
        playerQueueIndex = -1;
        playerPlaying = false;
        playerLoading = false;
        playerCurrentTime = 0;
        playerDuration = 0;
        activityMessage = 'Stopped playback because the file was removed from the Napstr folder';
      } else if (currentTrack) {
        playerQueue = queueForTrack(currentTrack, playerMode);
        playerQueueIndex = playerQueue.findIndex((item) => item.fileId === currentTrack?.fileId);
      }
      syncResultLocality();
    } catch { /* the next folder-watch or transfer poll will retry */ }
  }

  async function connectNetwork() {
    if (!nativeReady) return;
    activityMessage = 'Connecting to Nostr relays and opening encrypted inbox…';
    try {
      const status = await invoke<NetworkStatus>('start_network');
      applyNetworkStatus(status);
      activityMessage = `Nostr connected · loading the most available audio from ${status.relayCount} relay(s)…`;
      await search();
      if (status.torError) activityMessage = `Tor failed: ${status.torError} · click the connection panel to retry`;
    } catch (error) {
      networkConnected = false;
      networkError = String(error);
      activityMessage = `Network unavailable: ${String(error)}`;
    }
  }

  function applyNetworkStatus(status: NetworkStatus) {
    networkConnected = status.connected;
    torRunning = status.torRunning;
    torStarting = status.torStarting;
    torProgress = status.torProgress;
    torError = status.torError;
    identityNpub = status.npub;
    networkError = status.error;
  }

  function torStatusLabel() {
    if (torRunning) return 'Tor connected';
    if (torError) return 'Tor failed';
    if (torStarting && torProgress > 0) return `Tor connecting ${torProgress}%`;
    return nativeReady ? 'Tor connecting' : 'Tor unavailable';
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
        resultsAreNetwork = true;
        activityMessage = `${results.length} globally aggregated file ID(s), ranked by active seeders`;
      } catch (error) { activityMessage = `Global search failed: ${String(error)}`; }
    } else if (nativeReady) {
      try {
        const matches = await invoke<NativeFile[]>('search_catalog', { query: query.trim() });
        results = mapFiles(matches.filter((item) => minimumSources <= 1 && item.size <= maximumBytes() && matchesType(item.mime, item.format)));
        resultsAreNetwork = false;
        activityMessage = `${results.length} local match(es) found`;
      } catch (error) { activityMessage = `Search failed: ${String(error)}`; }
    }
    selected = results[0] ?? null;
    selectedSource = 0;
  }

  async function startDownload() {
    const target = selected;
    if (!target) return;
    if (nativeReady && isLocalFile(target.fileId)) {
      await playAudio(target.fileId, target.name);
      return;
    }
    const activeTransfer = transfers.find((item) => item.fileId === target.fileId && isActiveTransfer(item));
    if (activeTransfer || startingDownloads.has(target.fileId)) {
      activityMessage = `${target.name} is already downloading`;
      return;
    }
    if (nativeReady) {
      const sources = target.sourceDetails ?? [];
      if (!sources.length) { activityMessage = 'No seeder is available for this file'; return; }
      startingDownloads = new Set(startingDownloads).add(target.fileId);
      transfers = [{
        id: Date.now(), fileId: target.fileId, name: target.name, size: target.size,
        speed: 'Contacting seeders…', progress: 0, status: 'Sending encrypted NIP-17 request', destination: ''
      }, ...transfers];
      const candidateCount = Math.min(sources.length, 3);
      activityMessage = `Racing ${candidateCount} seeder${candidateCount === 1 ? '' : 's'} for the fastest Tor connection…`;
      try {
        await invoke('request_network_download', { fileId: target.fileId, sourcePubkeys: sources.map((source) => source.pubkey) });
        transfers = mapTransfers(await invoke<NativeTransfer[]>('get_transfers'));
        activityMessage = 'Seeder race started · the fastest responsive source will stream the file';
      } catch (error) {
        try { transfers = mapTransfers(await invoke<NativeTransfer[]>('get_transfers')); }
        catch { transfers = transfers.filter((item) => item.fileId !== target.fileId); }
        activityMessage = `Request failed: ${String(error)}`;
      } finally {
        const nextStarting = new Set(startingDownloads);
        nextStarting.delete(target.fileId);
        startingDownloads = nextStarting;
      }
      return;
    }
    transfers = [
      ...transfers,
      { id: Date.now(), fileId: target.fileId, name: `${target.name}.${target.format.toLowerCase()}`, size: target.size, speed: 'Negotiating…', progress: 2, status: 'Requesting private Tor session', destination: '' }
    ];
  }

  async function playSelectedAudio() {
    if (!selected || !isLocalFile(selected.fileId)) return;
    await playAudio(selected.fileId, selected.name);
  }

  async function playSelectedSharedAudio() {
    if (!selectedShared) return;
    await playAudio(selectedShared.fileId, selectedShared.filename);
  }

  async function playSelectedFolder() {
    if (!selectedShared) return;
    await playAudio(selectedShared.fileId, selectedShared.filename, 'folder');
  }

  async function playAllSongs() {
    const first = selectedShared ?? visibleSharedFiles()[0] ?? sharedFiles[0];
    if (!first) return;
    await playAudio(first.fileId, first.filename, 'all');
  }

  async function activateSelected() {
    if (nativeReady && selected && isLocalFile(selected.fileId)) await playSelectedAudio();
    else await startDownload();
  }

  function blockSelectedFile() {
    if (!nativeReady || !selected?.remote) return;
    blockConfirmation = { kind: 'file', fileId: selected.fileId, label: selected.name };
  }

  function blockSelectedUser() {
    const source = selected?.sourceDetails?.[selectedSource];
    if (!nativeReady || !source) return;
    blockConfirmation = { kind: 'user', pubkey: source.pubkey, label: source.displayName };
  }

  async function confirmBlock() {
    if (!blockConfirmation || blockInProgress) return;
    const target = blockConfirmation;
    blockInProgress = true;
    try {
      if (target.kind === 'file') {
        await invoke('block_file', { fileId: target.fileId });
        activityMessage = 'File hash blocked locally';
      } else {
        await invoke('block_user', { pubkey: target.pubkey });
        activityMessage = 'Nostr publisher blocked locally';
      }
      blockConfirmation = null;
      await search();
    } catch (error) {
      activityMessage = `Could not block ${target.kind}: ${String(error)}`;
    } finally {
      blockInProgress = false;
    }
  }

  async function removeTransfer(id: number) {
    if (nativeReady) {
      try { await invoke('cancel_transfer', { id }); await invoke('remove_transfer', { id }); } catch (error) { activityMessage = `Could not remove transfer: ${String(error)}`; }
    }
    transfers = transfers.filter((transfer) => transfer.id !== id);
    if (nativeReady) await refreshLocalLibrary();
  }

  async function clearFinishedTransfers() {
    const finished = transfers.filter(isFinishedTransfer);
    if (!finished.length) return;
    const removed = new Set<number>();
    for (const transfer of finished) {
      try {
        if (nativeReady) await invoke('remove_transfer', { id: transfer.id });
        removed.add(transfer.id);
      } catch (error) {
        activityMessage = `Could not clear every finished transfer: ${String(error)}`;
        break;
      }
    }
    transfers = transfers.filter((transfer) => !removed.has(transfer.id));
    if (removed.size === finished.length) activityMessage = `Cleared ${removed.size} finished transfer${removed.size === 1 ? '' : 's'}`;
    if (nativeReady) await refreshLocalLibrary();
  }

  async function togglePause() {
    paused = !paused;
    if (nativeReady) {
      try { await invoke('set_downloads_paused', { paused }); activityMessage = paused ? 'All active downloads paused' : 'Downloads resumed'; }
      catch (error) { activityMessage = `Could not change download state: ${String(error)}`; }
    }
  }

  async function chooseNapstrFolder() {
    if (!nativeReady) { activityMessage = 'Folder selection is available in the packaged desktop app'; return; }
    try {
      const selectedPath = await open({ directory: true, multiple: false, title: 'Choose the folder Napstr uses for downloads and sharing', defaultPath: napstrFolder || undefined });
      if (!selectedPath || Array.isArray(selectedPath)) return;
      activityMessage = 'Indexing files and calculating SHA-256 hashes…';
      const report = await invoke<{ fileCount: number; totalBytes: number; errors: string[] }>('set_napstr_folder', { path: selectedPath });
      await refreshSnapshot();
      if (networkConnected) await invoke('publish_catalogue');
      activityMessage = `Indexed ${report.fileCount} file(s), ${readableSize(report.totalBytes)}${report.errors.length ? ` · ${report.errors.length} skipped` : ''}`;
    } catch (error) { activityMessage = `Folder selection failed: ${String(error)}`; }
  }

  async function openNapstrFolder() {
    if (!nativeReady) return;
    try { await invoke('open_napstr_folder'); }
    catch (error) { activityMessage = `Could not open Napstr folder: ${String(error)}`; }
  }

  async function rescanSharedFolder() {
    if (!nativeReady) return;
    activityMessage = 'Rescanning Napstr folder…';
    try {
      const report = await invoke<{ fileCount: number; totalBytes: number }>('rescan_napstr_folder');
      await refreshSnapshot();
      if (networkConnected) await invoke('publish_catalogue');
      activityMessage = `Indexed ${report.fileCount} file(s), ${readableSize(report.totalBytes)}`;
    } catch (error) { activityMessage = `Rescan failed: ${String(error)}`; }
  }

  async function persistSettings() {
    if (!nativeReady) return;
    try {
      applySnapshot(await invoke<Snapshot>('save_settings', { settings: { napstrFolder, nostrRelays, displayName, profileAbout, profilePicture } }));
      if (networkConnected) await invoke('publish_profile');
      activityMessage = networkConnected ? 'Settings saved and profile published' : 'Settings saved';
    } catch (error) { activityMessage = `Could not save settings: ${String(error)}`; }
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
    return typeof window === 'undefined' ? 300 : Math.max(80, window.innerHeight - 395);
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
    const savedPlayerMode = window.localStorage.getItem('napstr-player-mode');
    if (savedPlayerMode === 'single' || savedPlayerMode === 'folder' || savedPlayerMode === 'all') playerMode = savedPlayerMode;
    const savedPlayerVolume = Number(window.localStorage.getItem('napstr-player-volume'));
    if (Number.isFinite(savedPlayerVolume) && savedPlayerVolume >= 0 && savedPlayerVolume <= 1) playerVolume = savedPlayerVolume;
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
        const previousTorError = torError;
        applyNetworkStatus(status);
        if (status.torError && status.torError !== previousTorError) {
          activityMessage = `Tor failed: ${status.torError} · click the connection panel to retry`;
        }
      }).catch(() => {});
    }, 5000);
    const transferTimer = window.setInterval(() => {
      if (nativeReady) {
        invoke<NativeTransfer[]>('get_transfers').then(async (items) => {
          const previouslyComplete = new Set(transfers.filter(isCompleteTransfer).map((transfer) => transfer.fileId));
          const updated = mapTransfers(items);
          const newlyComplete = updated.filter((transfer) => isCompleteTransfer(transfer) && !previouslyComplete.has(transfer.fileId));
          const vanishedActive = transfers.filter((transfer) => !startingDownloads.has(transfer.fileId) && isActiveTransfer(transfer) && !updated.some((item) => item.id === transfer.id));
          const optimistic = transfers.filter((transfer) => startingDownloads.has(transfer.fileId) && !updated.some((item) => item.fileId === transfer.fileId));
          transfers = [...optimistic, ...updated];
          if (newlyComplete.length || vanishedActive.length) {
            await refreshLocalLibrary();
            const latest = newlyComplete[0] ?? vanishedActive[0];
            activityMessage = `${latest.name} downloaded, verified, and ready to play`;
          }
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
    const libraryTimer = window.setInterval(() => {
      if (nativeReady) refreshLocalLibrary();
    }, 2000);
    const playerTimer = window.setInterval(() => {
      if (!nativeReady || !currentTrack || playerLoading) return;
      invoke<PlaybackStatus>('audio_status').then((status) => {
        const naturallyEnded = status.fileId === currentTrack?.fileId && status.ended && !playerEnded;
        applyPlaybackStatus(status);
        if (naturallyEnded) void playerTrackEnded();
      }).catch(() => {});
    }, 250);
    return () => {
      clearInterval(clockTimer);
      clearInterval(networkTimer);
      clearInterval(transferTimer);
      clearInterval(libraryTimer);
      clearInterval(playerTimer);
      window.removeEventListener('resize', clampTransferPane);
      stopTransferResize();
      if (nativeReady && currentTrack) invoke<PlaybackStatus>('stop_audio').catch(() => {});
    };
  });
</script>

<svelte:head><title>Napstr - own your music again</title></svelte:head>

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
      <button class="connection-box" onclick={connectNetwork} title={torError || networkError || 'Reconnect Nostr and Tor'}>
        <span class="connection-status"><i class:amber={!networkConnected} class="led"></i><strong>{networkConnected ? 'Nostr connected' : nativeReady ? 'Connect Nostr' : 'Preview mode'}</strong></span>
        <span class="connection-status"><i class:amber={!torRunning} class:error={Boolean(torError)} class="led"></i><strong>{torStatusLabel()}</strong></span>
      </button>
      <button class="tool-button help-button" onclick={() => (aboutOpen = true)}><span class="tool-icon">?</span><span>About</span></button>
    </div>

    <div class="network-strip">
      <span class="network-pulse">▥</span>
      <span>{activityMessage}</span>
      <span class="strip-right">{nativeReady ? displayName : 'demo@napstr'} <i class:amber={!nativeReady} class="led"></i></span>
    </div>

    <section class="player-bar" aria-label="Napstr audio player">
      <div class="player-display">
        <span class:playing={playerPlaying} class="player-led">{playerLoading ? '···' : playerPlaying ? '▶' : '■'}</span>
        <div><strong>{currentTrack?.name ?? 'No track selected'}</strong><small>{currentTrack ? `${currentTrack.artist || 'Unknown artist'} · ${folderName(currentTrack.folder)}` : 'Choose a local song to begin'}</small></div>
      </div>
      <div class="player-controls">
        <button onclick={previousPlayerTrack} disabled={!currentTrack || playerLoading} title="Previous track">|◀</button>
        <button class="player-primary" onclick={togglePlayer} disabled={playerLoading} title={playerPlaying ? 'Pause' : 'Play'}>{playerLoading ? '…' : playerPlaying ? 'Ⅱ' : '▶'}</button>
        <button onclick={stopPlayer} disabled={!currentTrack || playerLoading} title="Stop">■</button>
        <button onclick={nextPlayerTrack} disabled={playerLoading || playerQueueIndex < 0 || playerQueueIndex + 1 >= playerQueue.length} title="Next track">▶|</button>
      </div>
      <div class="player-seek">
        <input aria-label="Track position" type="range" min="0" max={Math.max(0, playerDuration || 0)} step="0.1" value={playerCurrentTime} oninput={seekPlayer} disabled={!currentTrack} />
        <span>{formatPlayerTime(playerCurrentTime)} / {formatPlayerTime(playerDuration)}</span>
      </div>
      <label class="player-mode">After track
        <select bind:value={playerMode} onchange={changePlayerMode}>
          <option value="single">Stop</option>
          <option value="folder">Play folder</option>
          <option value="all">Play all</option>
        </select>
      </label>
      <label class="player-volume">Vol <input aria-label="Volume" type="range" min="0" max="1" step="0.05" value={playerVolume} oninput={changePlayerVolume} /></label>
    </section>

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
                  {#if !isLocalFile(selected.fileId)}
                    {#each selected.sourceDetails ?? [] as source, index}
                      <button class:selected-source={selectedSource === index} class="source-row" onclick={() => (selectedSource = index)}><span class="user-icon">☺</span><b>{source.displayName}</b><small>{source.npub.slice(0, 12)}…</small><span class="online"><i></i> Seeding</span></button>
                    {/each}
                  {:else}
                    <div><span class="user-icon">☺</span><b>This computer</b><small>Local</small><span class="online"><i></i> Ready</span></div>
                  {/if}
                </div>
              </fieldset>
              <div class="detail-actions">{#if !isLocalFile(selected.fileId)}<button class="classic-button primary" disabled={startingDownloads.has(selected.fileId)} onclick={startDownload}>{startingDownloads.has(selected.fileId) ? '… Requesting' : '⇩ Download'}</button><button class="classic-button" onclick={() => (sourceProfile = selected?.sourceDetails?.[selectedSource] ?? null)}>View profile</button>{:else}<button class="classic-button primary" onclick={playSelectedAudio}>▶ Play</button><button class="classic-button" onclick={openNapstrFolder}>Open folder</button>{/if}</div>
              {#if !isLocalFile(selected.fileId)}<div class="detail-actions moderation-actions"><button class="classic-button" onclick={blockSelectedFile}>Block file</button><button class="classic-button" onclick={blockSelectedUser}>Block user</button></div>{/if}
              {#if !isLocalFile(selected.fileId)}<p class="privacy-note"><span>♜</span> Transfer will use the seeder’s private, app-session Tor onion service.</p>{:else}<p class="privacy-note"><span>♬</span> Downloaded and verified · ready to play from your Napstr folder.</p>{/if}
            {:else}<p class="empty-state">Select a result to see active seeders.</p>{/if}
          </aside>
        </div>
      {:else if activeView === 'Downloads'}
        <section class="full-panel">
          <div class="panel-title"><span></span><b>Download Manager</b><span></span></div>
          <div class="actionbar"><button class="classic-button" onclick={togglePause}>{paused ? '▶ Resume all' : 'Ⅱ Pause all'}</button><button class="classic-button" onclick={openNapstrFolder}>Open Napstr folder</button><button class="classic-button" onclick={clearFinishedTransfers} disabled={!transfers.some(isFinishedTransfer)}>Clear finished</button><div class="spacer"></div><span>{transfers.filter(isActiveTransfer).length} active · {transfers.filter(isCompleteTransfer).length} ready to play</span></div>
          <table class="file-table download-table"><thead><tr><th>Download order</th><th>Progress</th><th>Size</th><th>Speed</th><th>Status</th><th></th></tr></thead><tbody>
            {#each transfers as transfer}
              <tr class:transfer-complete={isCompleteTransfer(transfer)} ondblclick={() => { if (isCompleteTransfer(transfer)) playAudio(transfer.fileId, transfer.name); }}><td><span class="download-arrow">{isCompleteTransfer(transfer) ? '▶' : '⇩'}</span>{transfer.name}</td><td><div class="progress"><span style={`width:${transfer.progress}%`}></span><b>{Math.round(transfer.progress)}%</b></div></td><td>{transfer.size}</td><td>{isCompleteTransfer(transfer) ? 'Local' : transfer.speed}</td><td>{isCompleteTransfer(transfer) ? 'Ready to play' : transfer.status}</td><td class="transfer-actions">{#if isCompleteTransfer(transfer)}<button class="classic-button transfer-play" onclick={(event) => { event.stopPropagation(); playAudio(transfer.fileId, transfer.name); }} title="Play verified audio">▶ Play</button>{/if}<button class="tiny-button" onclick={(event) => { event.stopPropagation(); removeTransfer(transfer.id); }} title="Remove from this list">×</button></td></tr>
            {/each}
          </tbody></table>
          {#if transfers.length === 0}<p class="empty-state">There are no downloads in the queue.</p>{/if}
        </section>
      {:else if activeView === 'Shared'}
        <section class="full-panel">
          <div class="panel-title"><span></span><b>My Shared Files</b><span></span></div>
          <div class="actionbar"><button class="classic-button" onclick={rescanSharedFolder}>↻ Rescan</button><button class="classic-button" onclick={openNapstrFolder}>Open folder</button><button class="classic-button" onclick={playSelectedSharedAudio} disabled={!selectedShared}>▶ Play</button><button class="classic-button" onclick={playSelectedFolder} disabled={!selectedShared}>▶ Play folder</button><button class="classic-button primary" onclick={playAllSongs} disabled={!sharedFiles.length}>▶ Play all</button><div class="spacer"></div><span>Sharing {sharedFiles.length} files · {readableSize(indexedBytes)}</span></div>
          <div class="folder-path"><b>Napstr folder:</b><input value={napstrFolder || 'No folder selected'} readonly /><button class="classic-button" onclick={chooseNapstrFolder}>Browse…</button></div>
          <div class="library-filter"><label>View folder: <select bind:value={libraryFolderView}><option value="*">All folders</option>{#each libraryFolders() as folder}<option value={folder}>{folderName(folder)}</option>{/each}</select></label><span>{visibleSharedFiles().length} song{visibleSharedFiles().length === 1 ? '' : 's'} shown</span></div>
          <table class="file-table shared-table"><thead><tr><th>Name</th><th>Folder</th><th>Size</th><th>Catalogue</th><th>Active peers</th></tr></thead><tbody>{#each visibleSharedFiles() as file}<tr class:selected={selectedShared?.fileId === file.fileId} onclick={() => (selectedShared = { ...file })} ondblclick={() => playAudio(file.fileId, file.name)}><td><span class="file-icon">▶</span>{file.name}</td><td>{folderName(file.folder)}</td><td>{file.readableSize}</td><td><span class:amber={!networkConnected} class="led"></span>{networkConnected ? 'Published' : 'Indexed'}</td><td>{file.peers}</td></tr>{/each}</tbody></table>
          <p class="privacy-note wide"><span>♜</span> Only validated MP3, FLAC, WAV, Ogg Vorbis, and Opus audio is indexed recursively. Subfolders become player folders; folder names remain local and are not published. Embedded cover artwork is allowed.</p>
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
          <fieldset><legend>Files</legend><label>Downloads and shared audio <input value={napstrFolder} readonly /><button class="classic-button" onclick={chooseNapstrFolder}>Browse…</button></label><label>Transfer mode <select disabled><option>Whole file</option></select></label><label><input type="checkbox" checked disabled /> Downloaded audio is automatically shared</label><label><input type="checkbox" checked disabled /> Verify the complete file with SHA-256</label></fieldset>
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
      <div class="dock-title"><span></span><b>Transfer Manager</b><span></span><button class="dock-clear" onclick={clearFinishedTransfers} disabled={!transfers.some(isFinishedTransfer)}>Clear finished</button><button onclick={() => (activeView = 'Downloads')} title="Open Download Manager">□</button></div>
      <div class="mini-transfers">
        {#each transfers.slice(0, 2) as transfer}
          <div class:transfer-complete={isCompleteTransfer(transfer)} class="mini-row">{#if isCompleteTransfer(transfer)}<button class="mini-play" onclick={() => playAudio(transfer.fileId, transfer.name)} title="Play verified audio">▶</button>{:else}<span class="download-arrow">⇩</span>{/if}<span class="mini-name">{transfer.name}</span><div class="progress"><span style={`width:${transfer.progress}%`}></span></div><span>{transfer.size}</span><span>{isCompleteTransfer(transfer) ? 'Ready' : transfer.speed}</span></div>
        {/each}
      </div>
    </section>

    <footer class="statusbar"><span>{activityMessage}</span><span><i class:amber={!networkConnected} class="led"></i> Nostr {networkConnected ? 'online' : 'offline'}</span><span title={torError}>♜ Tor: {torRunning ? 'ready' : torError ? 'failed' : torStarting && torProgress > 0 ? `${torProgress}%` : 'starting'}</span><span class="status-clock">{clock}</span></footer>
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

  {#if blockConfirmation}
    <div class="modal-backdrop" role="presentation" onclick={() => { if (!blockInProgress) blockConfirmation = null; }}>
      <dialog class="dialog confirm-dialog" open aria-label="Confirm block" onclick={(e) => e.stopPropagation()} onkeydown={(e) => { if (e.key === 'Escape' && !blockInProgress) blockConfirmation = null; }}>
        <header class="titlebar"><div class="title-left"><span class="app-icon">!</span><span>Confirm block</span></div><div class="window-controls"><button disabled={blockInProgress} onclick={() => (blockConfirmation = null)}>×</button></div></header>
        <div class="dialog-body"><div class="confirm-icon">!</div><div><h3>Are you sure?</h3>{#if blockConfirmation.kind === 'file'}<p>Block <strong>{blockConfirmation.label}</strong>?</p><p>Every seeder offering these exact file bytes will be hidden.</p>{:else}<p>Block <strong>{blockConfirmation.label}</strong>?</p><p>Their catalogue entries and download requests will be ignored.</p>{/if}</div></div>
        <div class="dialog-actions"><button class="classic-button primary" disabled={blockInProgress} onclick={confirmBlock}>{blockInProgress ? 'Blocking…' : 'Block'}</button><button class="classic-button" disabled={blockInProgress} onclick={() => (blockConfirmation = null)}>Cancel</button></div>
      </dialog>
    </div>
  {/if}
</main>
