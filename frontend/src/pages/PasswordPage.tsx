interface PasswordPageProps {
  shareKey: string;
}

export function PasswordPage({ shareKey }: PasswordPageProps) {
  return (
    <main className="container" style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', height: '100dvh', flexDirection: 'column' }}>
      <div style={{ background: '#333', padding: '2rem', borderRadius: '8px', boxShadow: '0 4px 6px rgba(0,0,0,0.1)', textAlign: 'center' }}>
        <form id="unlock" method="post" action="/share/unlock" style={{ display: 'flex', flexDirection: 'column', gap: '1rem' }}>
          <input
            type="password"
            name="password"
            placeholder="Password"
            aria-label="Password"
            required
            autoFocus
            style={{ padding: '0.5rem', fontSize: '1rem', border: '1px solid #555', borderRadius: '4px', background: '#222', color: '#fff' }}
          />
          <input type="hidden" name="key" value={shareKey} />
          <button type="submit" style={{ padding: '0.5rem 1rem', fontSize: '1rem', background: '#007bff', color: '#fff', border: 'none', borderRadius: '4px', cursor: 'pointer' }}>
            Unlock
          </button>
        </form>
      </div>
    </main>
  );
}
