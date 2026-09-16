// The browser half of the bliti client (BLI-WEB): Web Bluetooth, the camera, and the interface.
//
// Everything with protocol in it lives in the wasm module: reading a sticker, matching an
// advertisement, the handshake, the streams, and the messages. This file only drives the browser
// APIs and hands their bytes across.

import init, {
	start,
	Sticker,
	Channel,
	service_uuid,
	client_tx_uuid,
	device_tx_uuid,
} from './pkg/bliti_web.js';

const $ = (id) => document.getElementById(id);

const state = {
	sticker: null,
	device: null,
	channel: null,
};

function log(message) {
	$('log-section').hidden = false;
	const line = document.createElement('p');
	line.textContent = message;
	$('log').append(line);
	$('log').scrollTop = $('log').scrollHeight;
}

function refuse(why) {
	$('unsupported').hidden = false;
	$('unsupported-why').textContent = why;
}

// A sticker has been read, by whichever path. Both paths land here, and the application treats the
// payload identically once read.
function stickerRead(sticker) {
	state.sticker = sticker;
	$('read').hidden = true;
	$('found').hidden = false;
	$('found-human').textContent = sticker.human;
	$('read-error').textContent = '';
}

function readFrom(text) {
	try {
		stickerRead(new Sticker(text));
	} catch (error) {
		$('read-error').textContent = error.message ?? String(error);
	}
}

// Finding the device the sticker belongs to (BLI-ADV, "Matching").
//
// The browser gives a chooser rather than the advertisements themselves, and the payload it filters
// on holds a salt that changes, so the chooser cannot be narrowed to one device ahead of time. It is
// filtered to devices carrying the bliti service, and the one the operator picks is then checked
// against the sticker before anything is sent to it.
async function findDevice() {
	const device = await navigator.bluetooth.requestDevice({
		filters: [{ services: [service_uuid()] }],
	});

	const advertised = device.name ? state.sticker.read_local_name(device.name) : undefined;
	if (!advertised) {
		throw new Error('That device is not advertising a bliti payload.');
	}
	if (advertised.version !== state.sticker.version) {
		throw new Error(`That device speaks bliti version ${advertised.version}, which this app does not read.`);
	}
	if (!advertised.matches) {
		throw new Error('That is a different bliti device. Pick the one whose sticker you read.');
	}
	return device;
}

function showIdentity(json) {
	const message = JSON.parse(json);
	if (message.type === 'unknown') {
		log(`The device did not understand something: ${message.reason}`);
		return;
	}
	if (message.type !== 'identity') {
		log(`The device sent a ${message.type} message.`);
		return;
	}

	const list = $('identity');
	list.replaceChildren();
	const add = (term, detail) => {
		const dt = document.createElement('dt');
		dt.textContent = term;
		const dd = document.createElement('dd');
		dd.textContent = detail;
		list.append(dt, dd);
	};
	add('Hostname', message.hostname);
	for (const address of message.addresses) {
		add(address.interface, address.address);
	}
}

async function connect() {
	$('connect').disabled = true;
	$('connect-status').textContent = 'Looking for the device...';
	try {
		const device = await findDevice();
		state.device = device;
		$('connect-status').textContent = 'Opening the channel...';

		const server = await device.gatt.connect();
		const service = await server.getPrimaryService(service_uuid());
		const clientTx = await service.getCharacteristic(client_tx_uuid());
		const deviceTx = await service.getCharacteristic(device_tx_uuid());

		// Writes are acknowledged, so the device is never sent more than it has taken. The bytes are
		// copied because the channel hands over a view it may reuse.
		const channel = new Channel(state.sticker, (bytes) => clientTx.writeValueWithResponse(bytes.slice()));
		state.channel = channel;

		deviceTx.addEventListener('characteristicvaluechanged', (event) => {
			channel.receive(new Uint8Array(event.target.value.buffer));
		});
		device.addEventListener('gattserverdisconnected', () => {
			log('The device disconnected.');
			$('session').hidden = true;
			$('connect').disabled = false;
			$('connect-status').textContent = 'Disconnected.';
		});

		// Subscribing is what opens a session: it is the point at which the device can send.
		await deviceTx.startNotifications();

		const first = await channel.connect(
			(json) => showIdentity(json),
			(why) => log(why ? `The device stopped reporting: ${why}` : 'The device stopped reporting.'),
		);

		showIdentity(first);
		$('connect-status').textContent = '';
		$('session').hidden = false;
		log('Channel open.');
	} catch (error) {
		// Picking nothing in the chooser is an ordinary thing to do, not a failure to report.
		const message = error.message ?? String(error);
		$('connect-status').textContent = error.name === 'NotFoundError' ? '' : message;
		$('connect').disabled = false;
		if (state.device?.gatt?.connected) {
			state.device.gatt.disconnect();
		}
	}
}

async function send() {
	const text = $('text').value.trim();
	if (!text) return;
	$('send').disabled = true;
	$('send-status').textContent = '';
	try {
		await state.channel.send_text(text);
		$('text').value = '';
		log(`Sent: ${text}`);
	} catch (error) {
		$('send-status').textContent = error.message ?? String(error);
	} finally {
		$('send').disabled = false;
	}
}

// Capturing a code with the camera, for provisioning several devices in one session without leaving
// and re-entering the application for each one.
async function scan() {
	const video = $('preview');
	$('read-error').textContent = '';
	let stream;
	try {
		stream = await navigator.mediaDevices.getUserMedia({
			video: { facingMode: 'environment' },
		});
	} catch (error) {
		$('read-error').textContent = `The camera is not available: ${error.message ?? error}`;
		return;
	}

	const detector = new BarcodeDetector({ formats: ['qr_code'] });
	let rejected = null;
	video.hidden = false;
	video.srcObject = stream;
	await video.play();

	const stop = () => {
		video.hidden = true;
		video.srcObject = null;
		for (const track of stream.getTracks()) track.stop();
	};

	while (video.srcObject) {
		let codes = [];
		try {
			codes = await detector.detect(video);
		} catch {
			// A frame that cannot be read is not worth reporting; the next one is along shortly.
		}
		for (const code of codes) {
			try {
				const sticker = new Sticker(code.rawValue);
				stop();
				stickerRead(sticker);
				return;
			} catch (error) {
				// A code that is not a bliti sticker does not stop the camera, because the next thing
				// in frame may well be one. But it is reported: saying nothing is indistinguishable
				// from a code the camera cannot read at all, which leaves the operator holding a
				// sticker up to a camera that looks broken. Reported once per code rather than on
				// every frame it stays in view.
				if (code.rawValue !== rejected) {
					rejected = code.rawValue;
					$('read-error').textContent = error.message ?? String(error);
				}
			}
		}
		await new Promise((resolve) => setTimeout(resolve, 200));
	}
}

async function main() {
	await init();
	start();

	if (!window.isSecureContext) {
		refuse('This page needs a secure context. Open it over https, or over localhost while developing.');
		return;
	}
	if (!navigator.bluetooth) {
		refuse('This browser does not offer Web Bluetooth. Chrome on Android is the tested one.');
		return;
	}
	if ('BarcodeDetector' in window) {
		$('scan').hidden = false;
	}

	$('use-typed').addEventListener('click', () => readFrom($('typed').value));
	$('typed').addEventListener('keydown', (event) => {
		if (event.key === 'Enter') readFrom($('typed').value);
	});
	$('scan').addEventListener('click', scan);
	$('connect').addEventListener('click', connect);
	$('send').addEventListener('click', send);
	$('text').addEventListener('keydown', (event) => {
		if (event.key === 'Enter') send();
	});
	$('forget').addEventListener('click', () => {
		state.sticker = null;
		$('found').hidden = true;
		$('read').hidden = false;
	});

	// Following the link opens the application with the payload already in the fragment. It is read
	// here in the browser and goes no further.
	if (location.hash.length > 1) {
		readFrom(location.hash);
	}
}

main();
