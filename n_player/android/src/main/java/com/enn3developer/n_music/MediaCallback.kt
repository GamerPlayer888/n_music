package com.enn3developer.n_music

import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Bundle
import androidx.media3.session.R
import com.enn3developer.n_music.MainActivity.Companion.ACTIONS
import com.enn3developer.n_music.MainActivity.Companion.CUSTOM_REPLAY_ON
import com.enn3developer.n_music.MainActivity.Companion.CUSTOM_REPLAY_OFF

class MediaCallback(
    private val mediaSession: MediaSession,
    private val activity: MainActivity
) : MediaSession.Callback() {

    private external fun TogglePause()
    private external fun PlayNext()
    private external fun PlayPrevious()
    private external fun Seek(seek: Double)
    private external fun ToggleRepeat()

    override fun onPause() {
        TogglePause()
        val position = mediaSession.controller.playbackState?.position ?: 0L

        activity.playback?.setState(
            PlaybackState.STATE_PAUSED,
            position, 1.0f
        )

        mediaSession.setPlaybackState(activity.playback?.build())
        super.onPause()
    }

    override fun onPlay() {
        TogglePause()
        val position = mediaSession.controller.playbackState?.position ?: 0L

        activity.playback?.setState(
            PlaybackState.STATE_PLAYING,
            position, 1.0f
        )
        mediaSession.setPlaybackState(activity.playback?.build())
        super.onPlay()
    }

    override fun onSkipToNext() {
        PlayNext()
        super.onSkipToNext()
    }

    override fun onSkipToPrevious() {
        PlayPrevious()
        activity.playback?.setState(PlaybackState.STATE_PLAYING, 0L, 1.0f)
        mediaSession.setPlaybackState(activity.playback?.build())
        super.onSkipToPrevious()
    }

    override fun onSeekTo(pos: Long) {
        Seek((pos / 1000).toDouble())
        activity.playback?.setState(PlaybackState.STATE_PLAYING, pos, 1.0f)
        mediaSession.setPlaybackState(activity.playback?.build())
        super.onSeekTo(pos)
    }

    override fun onCustomAction(action: String, extras: Bundle?) {
        ToggleRepeat()
        val pos = mediaSession.controller.playbackState?.position ?: 0L
        val state = mediaSession.controller.playbackState?.state ?: PlaybackState.STATE_NONE

        activity.playback = PlaybackState.Builder().setActions(ACTIONS)

        when (action) {
            CUSTOM_REPLAY_OFF -> {
                activity.playback?.addCustomAction(
                    PlaybackState.CustomAction.Builder(
                        CUSTOM_REPLAY_ON,
                        "REPEAT ON",
                        R.drawable.media3_icon_repeat_all
                    ).build()
                )
            }
            CUSTOM_REPLAY_ON -> {
                activity.playback?.addCustomAction(
                    PlaybackState.CustomAction.Builder(
                        CUSTOM_REPLAY_OFF,
                        "REPEAT OFF",
                        R.drawable.media3_icon_repeat_off
                    ).build()
                )
            }
        }

        activity.playback?.setState(state, pos, 1.0f)

        val newState = activity.playback?.build()
        mediaSession.setPlaybackState(newState)

        super.onCustomAction(action, extras)
    }
}