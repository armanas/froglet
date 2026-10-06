// Public metadata only. No signing seed, runtime token or provider credential belongs here.
export const PUBLIC_DEMO = {
  providerId: 'c7a15140cc28833978197bdef7daf30e419502f186f59ff88584f453531ea894',
  origin: 'https://froglet-public-beta-20261006.fly.dev',
  computeOffer: 'execute.compute',
  catalogService: 'synthetic-terminology-demo',
  maxModuleBytes: 262144,
  maxInputBytes: 131072,
  maxMemoryBytes: 8388608,
  maxRuntimeMs: 2000,
  maxFuel: 50000000,
} as const;

export const PUBLIC_DEMO_PREFIX = '/api/public-demo';
