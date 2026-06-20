export function HomePage() {
  return (
    <div className="container" style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', height: '100dvh', background: '#262626', margin: 0 }}>
      <a href="https://github.com/alangrainger/immich-public-proxy">
        <img src="/images/ipp.svg" alt="Immich Public Proxy" style={{ maxWidth: '280px', height: '280px', opacity: 0.3 }} />
      </a>
    </div>
  );
}
