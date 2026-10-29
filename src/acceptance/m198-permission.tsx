// SPDX-License-Identifier: Apache-2.0
import { createRoot } from 'react-dom/client';
import '../styles/globals.css';
import { PermissionDialog } from '../components/PermissionDialog';
createRoot(document.getElementById('root')!).render(<PermissionDialog trusted request={{ intentId: 'synthetic-m198', toolCallId: 'synthetic-outside', toolName: 'bash', args: { command: "python3 - <<'PY'\nopen('../outside','w').write('bad')\nPY", _permission: { rule: 'trusted.heredoc.unproved', reason: '无法证明解释器仅访问已登记工作区内的文件；越界写入需要确认。' } } }} onAllow={() => {}} onDeny={() => {}} onAllowFullAccess={() => { throw new Error('Already trusted'); }} />);
