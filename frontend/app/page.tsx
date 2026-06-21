'use client';

import { useState } from "react";
import { Arbutus, Syncopate } from "next/font/google";
import { subscribeUser } from "@/app/actions";

const arbutus = Arbutus({
  subsets: ["latin"],
  weight: ["400"],
});

const syncopate = Syncopate({
  subsets: ["latin"],
  weight: ["700"],
});

export default function Home() {
  const [email, setEmail] = useState("");
  const [loading, setLoading] = useState(false);
  const [feedback, setFeedback] = useState<{ success?: boolean; message?: string } | null>(null);

  const handleSubmit = async (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    setLoading(true);
    setFeedback(null);

    const formData = new FormData(e.currentTarget);
    try {
      const response = await subscribeUser(null, formData);
      if (response.success) {
        setFeedback({ success: true, message: response.message });
        setEmail("");
      } else {
        setFeedback({ success: false, message: response.error });
      }
    } catch (err) {
      console.error(err);
      setFeedback({ success: false, message: "Something went wrong. Please try again." });
    } finally {
      setLoading(false);
    }
  };

  return (
    <main
  className="
    min-h-screen
    bg-[url('/backgrounds/manga-background.png')]
    bg-cover
    bg-[position:center_15%]
    md:bg-center
    flex
    items-center
    justify-center
    p-4
    mb-2
  "
>

<div
  className="
    flex flex-col
    items-center
    text-center
    w-full
    max-w-4xl
    px-4

    mt-[18vh]
    sm:mt-[15vh]
    md:mt-[10vh]
    lg:mt-[15vh]
    xl:mt-[20vh]
  "
>

        <h1 className={`${syncopate.className} text-neutral-200 text-3xl sm:text-4xl md:text-5xl font-bold tracking-[0.2em] mb-4`}>
          X7710
        </h1>

        <h2 className={`${arbutus.className} text-neutral-200 text-3xl sm:text-6xl md:text-9xl font-extrabold tracking-[0.2em] sm:tracking-[0.3em] mb-8`}>
          COMING SOON
        </h2>

        <p className="text-neutral-400 uppercase tracking-[0.2em] sm:tracking-[0.3em] text-[10px] sm:text-xs md:text-sm leading-6 sm:leading-8 mb-10">
          THE AGENT CONTROL LAYER FOR STELLAR
          <br />
          AGENTS NEED RULES
        </p>

        <p className="text-neutral-200 tracking-[0.2em] sm:tracking-[0.3em] text-xs sm:text-sm mb-6">
          BE THE FIRST TO KNOW
        </p>

        <form onSubmit={handleSubmit} className="flex flex-col sm:flex-row w-full max-w-2xl gap-3 sm:gap-0">
          <input
            type="email"
            name="email"
            required
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            disabled={loading}
            placeholder="ENTER YOUR EMAIL"
            className="flex-1 bg-transparent border border-neutral-600 px-6 py-4 text-neutral-200 outline-none placeholder:text-neutral-500 tracking-[0.1em] sm:tracking-[0.2em] text-center sm:text-left text-sm sm:text-base disabled:opacity-50"
          />

          <button
            type="submit"
            disabled={loading}
            className="bg-neutral-200 text-black px-8 py-4 tracking-[0.1em] sm:tracking-[0.2em] uppercase font-medium hover:bg-neutral-300 transition text-sm sm:text-base disabled:opacity-50"
          >
            {loading ? "Sending..." : "Notify Me"}
          </button>
        </form>

        {feedback && (
          <p className={`mt-4 text-xs sm:text-sm tracking-wider uppercase font-medium ${feedback.success ? 'text-white' : 'text-red-400'}`}>
            {feedback.message}
          </p>
        )}

      </div>
    </main>
  );
}