import Link from "next/link";

export default function NotFound() {
  return (
    <main className="landing-shell">
      <section className="landing-card">
        <div className="eyebrow">404 · Path not found</div>
        <h1>This market path does not exist.</h1>
        <p>Return to the lab to start a sealed practice run.</p>
        <Link className="primary-action" href="/">
          Return to the game
        </Link>
      </section>
    </main>
  );
}
