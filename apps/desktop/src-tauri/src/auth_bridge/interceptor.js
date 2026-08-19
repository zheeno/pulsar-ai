(function () {
  var SESSION_NONCE = "__PULSAR_SESSION_NONCE__";
  var ALLOWED_ORIGINS = __PULSAR_ALLOWED_ORIGINS__;
  var HEADER_NAMES = __PULSAR_HEADER_NAMES__;
  var WS_PARAM = __PULSAR_WS_PARAM__;
  var nativeFetch = window.fetch;

  function writeCookie(name, val) {
    try {
      if (typeof document === "undefined") return;
      document.cookie =
        name + "=" + encodeURIComponent(val) + "; path=/; SameSite=Lax";
    } catch (e) {}
  }

  function writeCaptureCookie(headerName, value) {
    writeCookie(
      "pulsar_ab",
      String(headerName) + "\x1e" + SESSION_NONCE + "\x1e" + String(value)
    );
  }

  try {
    if (!ALLOWED_ORIGINS || ALLOWED_ORIGINS.indexOf(String(location.origin)) === -1) {
      return;
    }
  } catch (e) {
    return;
  }

  if (window.fetch && window.fetch.__abWrapped) {
    return;
  }

  function relay(headerName, value) {
    if (!headerName || typeof value !== "string" || !value) {
      return;
    }
    try {
      writeCaptureCookie(headerName, value);
      var internals = window.__TAURI_INTERNALS__;
      if (!internals || typeof internals.invoke !== "function") {
        return;
      }
      var p = internals.invoke("auth_bridge_submit_candidate", {
        sessionNonce: SESSION_NONCE,
        headerName: String(headerName),
        value: String(value)
      });
      if (p && typeof p.catch === "function") {
        p.catch(function () {});
      }
    } catch (err) {}
  }

  function wanted(name) {
    if (!name) return false;
    var n = String(name).toLowerCase();
    for (var i = 0; i < HEADER_NAMES.length; i++) {
      if (HEADER_NAMES[i] === n) return true;
    }
    return false;
  }

  function scanHeaders(headers) {
    if (!headers) return;
    if (typeof Headers !== "undefined" && headers instanceof Headers) {
      headers.forEach(function (value, name) {
        if (wanted(name)) relay(String(name).toLowerCase(), value);
      });
      return;
    }
    if (Array.isArray(headers)) {
      for (var i = 0; i < headers.length; i++) {
        var pair = headers[i];
        if (pair && pair.length >= 2 && wanted(pair[0])) {
          relay(String(pair[0]).toLowerCase(), String(pair[1]));
        }
      }
      return;
    }
    if (typeof headers === "object") {
      for (var key in headers) {
        if (Object.prototype.hasOwnProperty.call(headers, key) && wanted(key)) {
          relay(String(key).toLowerCase(), String(headers[key]));
        }
      }
    }
  }

  function scanRequest(input, init) {
    if (input && typeof Request !== "undefined" && input instanceof Request) {
      scanHeaders(input.headers);
    }
    if (init && init.headers) {
      scanHeaders(init.headers);
    }
  }

  if (typeof nativeFetch === "function") {
    window.fetch = function (input, init) {
      try {
        scanRequest(input, init);
      } catch (e) {}
      return nativeFetch.apply(this, arguments);
    };
    window.fetch.__abWrapped = true;
  }

  var XHR = window.XMLHttpRequest;
  if (XHR && XHR.prototype) {
    var nativeOpen = XHR.prototype.open;
    var nativeSet = XHR.prototype.setRequestHeader;
    var nativeSend = XHR.prototype.send;
    var bag =
      typeof WeakMap === "function"
        ? new WeakMap()
        : null;

    function bagGet(xhr) {
      if (bag) {
        var cur = bag.get(xhr);
        if (!cur) {
          cur = [];
          bag.set(xhr, cur);
        }
        return cur;
      }
      return [];
    }

    XHR.prototype.open = function () {
      if (bag) bag.set(this, []);
      return nativeOpen.apply(this, arguments);
    };
    XHR.prototype.setRequestHeader = function (name, value) {
      try {
        if (wanted(name)) {
          if (bag) {
            bagGet(this).push([String(name).toLowerCase(), String(value)]);
          } else {
            relay(String(name).toLowerCase(), String(value));
          }
        }
      } catch (e) {}
      return nativeSet.apply(this, arguments);
    };
    XHR.prototype.send = function () {
      try {
        if (bag) {
          var items = bag.get(this) || [];
          for (var i = 0; i < items.length; i++) {
            relay(items[i][0], items[i][1]);
          }
        }
      } catch (e) {}
      return nativeSend.apply(this, arguments);
    };
  }

  if (WS_PARAM && typeof window.WebSocket === "function") {
    var NativeWS = window.WebSocket;
    window.WebSocket = function (url, protocols) {
      try {
        var href = typeof url === "string" ? url : String(url);
        var q = href.split("?")[1] || "";
        var parts = q.split("&");
        for (var i = 0; i < parts.length; i++) {
          var kv = parts[i].split("=");
          if (kv.length >= 2 && decodeURIComponent(kv[0]) === WS_PARAM) {
            relay("websocket", decodeURIComponent(kv.slice(1).join("=")));
          }
        }
      } catch (e) {}
      if (protocols === undefined) {
        return new NativeWS(url);
      }
      return new NativeWS(url, protocols);
    };
    window.WebSocket.prototype = NativeWS.prototype;
    window.WebSocket.CONNECTING = NativeWS.CONNECTING;
    window.WebSocket.OPEN = NativeWS.OPEN;
    window.WebSocket.CLOSING = NativeWS.CLOSING;
    window.WebSocket.CLOSED = NativeWS.CLOSED;
  }
})();
