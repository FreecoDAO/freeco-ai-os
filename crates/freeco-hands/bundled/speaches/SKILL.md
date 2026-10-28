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
- **dograh voice calls** — in dograh, choose *Speaches* as the transcriber and
  the voice, with base URL `http://127.0.0.1:8010/v1` and the same key.

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
