import urllib.request
class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_):
        return None


_opener = urllib.request.OpenerDirector()
for _handler in (urllib.request.ProxyHandler({}), urllib.request.HTTPHandler(),
                 urllib.request.HTTPDefaultErrorHandler(), urllib.request.HTTPErrorProcessor(), _NoRedirect()):
    _opener.add_handler(_handler)
