/** Presentation only. Never infer dataset contents from a service name. */
export function serviceName(id: string): string {
  return id.split(/[._-]+/).filter(Boolean).map(word => /^(hla|api|json|csv|sql|wasm)$/i.test(word) ? word.toUpperCase() : word[0].toUpperCase() + word.slice(1)).join(' ');
}

export function collectionsIn(schema: unknown): Record<string, string[]> {
  if (!schema || typeof schema !== 'object') return {};
  const raw = (schema as Record<string, unknown>)['x-froglet-collections'];
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return {};
  return Object.fromEntries(Object.entries(raw).filter(([name, columns]) => name.length <= 128 && Array.isArray(columns) && columns.every(column => typeof column === 'string' && column.length <= 128)));
}

export function serviceDescription(summary: unknown, id: string, schema?: unknown): string {
  const authored = typeof summary === 'string' ? summary.trim() : '';
  if (authored && authored !== `Shared ${id}` && !authored.startsWith('Query published catalog data:')) return authored.slice(0, 1000);
  const collections = Object.keys(collectionsIn(schema));
  if (collections.length) return `Explore ${collections.length} published ${collections.length === 1 ? 'table' : 'tables'}: ${collections.slice(0, 8).map(name => name.replaceAll('_', ' ')).join(', ')}${collections.length > 8 ? ', and more' : ''}. Select fields and filter rows to answer questions about this catalog.`;
  return 'Open the service details to inspect its inputs, outputs, and availability.';
}
