# Speaches — local speech

Speech-to-text and text-to-speech that run on this machine. It serves the
OpenAI audio API, so anything that speaks that API can use it without a cloud
account.

## Endpoints

Listens on `http://127.0.0.1:8010`. Every request needs the bearer token held
in `SPEACHES_API_KEY`.

- `POST /v1/audio/transcriptions` — speech to text (Whisper)
- `POST /v1/audio/translations` — speech to English text
- `POST /v1/audio/speech` — text to speech (Kokoro, Piper)
- `GET /v1/models` — what is downloaded and available
- `GET /health` — liveness

## Who uses it

- **FreEco.ai dictation and spoken replies** — in `config.toml`:

  ```toml
  [media]
  audio_provider = "openai"
  audio_base_url = "http://127.0.0.1:8010"
  tts_openai_base_url = "http://127.0.0.1:8010"
  local_audio_api_key_env = "SPEACHES_API_KEY"
  ```

  No `/v1` on the end: FreEco.ai appends `/v1/audio/...` itself.
  `local_audio_api_key_env` makes it send this service's key rather than your
  OpenAI key, which must never go to a service other than OpenAI.

  Pointing these at this local address also turns on `[media] local_only`
  unless it is set to `false`: FreEco.ai then never picks Groq, OpenAI or
  ElevenLabs from their keys, and if this service is down, dictation and
  spoken replies fail with an error naming `http://127.0.0.1:8010` instead of
  quietly sending audio to a cloud provider.
- **dograh voice calls** — dograh keeps its speech settings in its own
  database, set in its UI (Settings → models), not in the hand's environment,
  so they have to be chosen once there. Its default is cloud (Deepgram for
  speech-to-text, ElevenLabs for the voice). Choose *Local Models (Speaches)*
  for both, with:
  - base URL `http://host.containers.internal:8010/v1` — **not** dograh's
    suggested `http://localhost:8000/v1`: inside dograh's container
    `localhost` is the container itself, and port 8000 is dograh's own API;
  - API key: the value of `SPEACHES_API_KEY`;
  - transcriber model `Systran/faster-whisper-small` (dograh suggests the
    English-only `faster-distil-whisper-small.en`, which is not loaded here)
    and the call's language;
  - voice model and voice as listed by `GET /v1/models` and
    `GET /v1/audio/voices` here.

## Models

`Systran/faster-whisper-small` is loaded at start: multilingual, and good on
English, Ukrainian, German, French and Italian. Other models download on first
use and are kept in the `models` volume.

Text-to-speech voices vary by language. Kokoro covers English, French and
Italian among others; Piper has Ukrainian and German voices. Ask
`GET /v1/audio/voices` rather than assuming one exists.

## Privacy

Telemetry is off, and the only outbound traffic is downloading a model the
first time it is used. Audio and text never leave the machine.
